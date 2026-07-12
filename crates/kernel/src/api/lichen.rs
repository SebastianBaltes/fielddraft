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
        // PANIC-FEST: ein Panic in einer &mut-WASM-Methode setzt das
        // wasm-bindgen-Borrow-Flag dauerhaft — JEDER Folgeaufruf wirft dann
        // „recursive use of an object … unsafe aliasing" und der Viewport
        // ist tot (Betreiber-Report). Der Debug-Mesher darf scheitern,
        // aber nicht die Session vergiften: abfangen, Fehler loggen,
        // leeres Ergebnis liefern.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.lichen_mesh_impl(opts, None)
        })) {
            Ok(out) => out,
            Err(e) => {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "unbekannter Panic".into());
                // eprintln landet nativ auf stderr; im WASM sorgt der
                // panic-hook bereits für die Konsolen-Ausgabe des Panics
                // selbst — hier geht es nur um die Session-Rettung.
                eprintln!("lichenMesh panic: {msg}");
                vec![0.0, 0.0, 0.0]
            }
        }
    }

    /// Kern (testbar, per-Body-fähig). `body_filter = None` = ganze Szene.
    pub(crate) fn lichen_mesh_impl(
        &mut self,
        opts: LichenOptions,
        body_filter: Option<usize>,
    ) -> Vec<f32> {
        self.lichen_mesh_sized(opts, body_filter, None)
    }

    /// Kern mit optionalem SIZING-FIELD. Ist `opts.sizing_field` gesetzt und
    /// noch kein Feld übergeben, läuft die Pipeline ZWEIMAL: erst grob (um
    /// eine Stützstellen-Wolke zu bekommen), dann mit dem daraus gebauten,
    /// Lipschitz-geglätteten Dichtefeld für Ketten UND Wachstum.
    pub(crate) fn lichen_mesh_sized(
        &mut self,
        opts: LichenOptions,
        body_filter: Option<usize>,
        size: Option<&lichen::SizeField>,
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

            let h_local = h / s.max(1e-6);
            let min_spacing_local = opts.min_edge_spacing.max(0.0) / s.max(1e-6);
            let seg_len = |a: [f32; 3], b: [f32; 3]| -> f32 {
                ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
            };

            // Ecken (flag 3): globale Slots; Normale (Mittel der Flächen) folgt.
            //
            // MIKRO-FEATURE-KOLLAPS (Ticket 2e7eab20, „andersrum"): B-Rep-Ecken,
            // die näher beieinander liegen als der Min. Kantenabstand, teilen
            // sich EINEN Mesh-Vertex (Cluster-Repräsentant = erste Ecke). Solche
            // Häufungen entstehen z.B. bei streifenden Verschneidungen (die
            // r=30-Bohrung dieses Modells trifft die Box-Kante bei Radial-
            // abstand 29.998) — Topologie unterhalb der Mesh-Auflösung, die
            // U(Ecke) sonst auf µm drückt und Verdichtungs-Kaskaden auslöst.
            let mut corner_slot: HashMap<usize, u32> = HashMap::new();
            let mut cluster_reps: Vec<(u32, [f64; 3])> = Vec::new();
            for (vi, v) in brep.vertices.iter().enumerate() {
                let rep = cluster_reps.iter().find(|&&(_, rp)| {
                    let dx = v.position[0] - rp[0];
                    let dy = v.position[1] - rp[1];
                    let dz = v.position[2] - rp[2];
                    ((dx * dx + dy * dy + dz * dz) as f32).sqrt() < min_spacing_local
                });
                let slot = match rep {
                    Some(&(slot, _)) => slot,
                    None => {
                        let p = to_world(v.position);
                        let (d, _) = fg(p);
                        let slot = all.len() as u32;
                        all.push(LPoint {
                            p,
                            sdf: d,
                            n: [0.0, 0.0, 1.0],
                            u: h,
                            space: h,
                            flag: 3.0,
                        });
                        point_faces.push(Vec::new());
                        local_pos.push([
                            v.position[0] as f32,
                            v.position[1] as f32,
                            v.position[2] as f32,
                        ]);
                        cluster_reps.push((slot, v.position));
                        slot
                    }
                };
                corner_slot.insert(vi, slot);
            }

            // Kanten (flag 2): in LOCAL resamplen (§3.2 krümmungsgesteuert);
            // geordnete globale Index-Kette je Kante merken (für die Flächen-
            // Loops); chain_links = feste Feature-Kanten in G; Welt-Segmente für
            // R_BRep.
            // Kette einer Kante bauen: adaptives Resampling (mit Ecken-Rand-
            // Bedingungen fürs Grading) + 4-Punkte-Schutz für Ringe.
            // ε-SAMPLING DER KETTEN (2026-07-12): die B-Rep-Kanten folgten der
            // lfs-Abtastung NICHT — auf einer 1-mm-Platte blieb die Kette bei
            // 1,75 mm, während die Flächen auf 0,26 mm verfeinert wurden. Ein
            // Ketten-Punkt sitzt dann als grober Fremdkörper in einer dichten
            // Punktwolke: sein Delaunay-Fächer ist entartet, die leere Umkugel
            // eines JEDEN Dreiecks mit ihm enthält Flächen-Punkte, und der
            // einzige Kandidat, den die Umkugel noch zulässt, liegt auf der
            // GEGENSEITE der Platte (gemessen). Die Kette muss also mindestens
            // so fein sein wie die Flächen, die an ihr hängen.
            let edge_lfs_h = |raw_local: &[[f32; 3]]| -> f32 {
                // Mit SIZING-FIELD ist es die Vorgabe (es enthält lfs bereits).
                if let Some(sf) = size {
                    let mut m = h;
                    let step = (raw_local.len() / 16).max(1);
                    for (i, pl) in raw_local.iter().enumerate() {
                        if i % step != 0 {
                            continue;
                        }
                        let pw = to_world([pl[0] as f64, pl[1] as f64, pl[2] as f64]);
                        m = m.min(sf.at(pw));
                    }
                    return (m / s.max(1e-6)).min(h_local);
                }
                if opts.lfs_factor <= 0.0 {
                    return h_local;
                }
                let cap_lfs = h / opts.lfs_factor;
                let mut m = h;
                let step = (raw_local.len() / 8).max(1);
                for (i, pl) in raw_local.iter().enumerate() {
                    if i % step != 0 {
                        continue;
                    }
                    let pw = to_world([pl[0] as f64, pl[1] as f64, pl[2] as f64]);
                    // An einer B-Rep-Kante ist ∇f die Winkelhalbierende — als
                    // Marschrichtung ins Material völlig ausreichend.
                    let n = lichen::unit(lichen::robust_grad(pw, &fg));
                    // Startschritt wie im Wachstum (grob — s. lichen::local_radius).
                    let t0 = 0.02 * cap_lfs;
                    let lfs = lichen::reach_inner(pw, n, cap_lfs, t0, &fg)
                        .min(lichen::reach_outer_flat(pw, n, cap_lfs, t0, &fg));
                    if lfs < cap_lfs {
                        m = m.min(opts.lfs_factor * lfs);
                    }
                }
                (m / s.max(1e-6)).min(h_local)
            };
            let build_chain = |raw_local: &[[f32; 3]], closed: bool, bounds: (f32, f32)| {
                let mut chain = lichen::resample_edge_by_curvature(
                    raw_local,
                    edge_lfs_h(raw_local),
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
                // RING-NAHT-KOHÄRENZ (Betreiber-Demo Kugel∩Gyroid): der
                // Resampler sieht den Ring als OFFENE Polyline — am Nahtstoß
                // letzter→erster Punkt kann ein Mini-Restsegment übrig
                // bleiben (gemessen 0.35 neben 2.82), dessen U-Deckel
                // (bound_pair nimmt das Minimum der Nachbarsegmente) die
                // Überdeckungs-Invariante u_a+u_b ≥ d des ersten/letzten
                // regulären Segments reißt. Letzte Emissionen verwerfen, bis
                // die drei Naht-Paare die Überdeckungsregel erfüllen — der
                // Naht-Ort auf einem Ring ist ohnehin ein Abtast-Artefakt.
                if closed && chain.len() >= 2 {
                    let dup = usize::from(chain.first() == chain.last());
                    loop {
                        let m = chain.len() - dup;
                        if m <= 4 {
                            break;
                        }
                        let d = |i: usize, j: usize| seg_len(chain[i], chain[j]);
                        let w = d(m - 1, 0);
                        let d0 = d(0, 1);
                        let d1 = d(1, (m - 1).min(2));
                        let dl = d(m - 2, m - 1);
                        let dl2 = d(m - 3, m - 2);
                        let ok_wrap = dl.min(w) + w.min(d0) >= w * 0.9995;
                        let ok_first = w.min(d0) + d0.min(d1) >= d0 * 0.9995;
                        let ok_last = dl2.min(dl) + dl.min(w) >= dl * 0.9995;
                        if ok_wrap && ok_first && ok_last {
                            break;
                        }
                        chain.remove(m - 1);
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
                /// Mikro-Feature-Kollaps: Kante verschwindet aus dem Mesh —
                /// offene Mikro-Kante innerhalb EINES Ecken-Clusters → nur der
                /// Repräsentant bleibt; Mikro-Ring (Umfang < 4·min_spacing) →
                /// ersatzlos (das Loch ist Sub-Resolution).
                collapse: bool,
            }
            let mut pedges: Vec<PEdge> = Vec::with_capacity(brep.edges.len());
            let mut corner_gap: HashMap<usize, f32> = HashMap::new();
            for e in brep.edges.iter() {
                let raw_local: Vec<[f32; 3]> = crate::brep::edge_sampling::sample_edge(&e.geometry)
                    .into_iter()
                    .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
                    .collect();
                let closed = e.start.is_none() && e.end.is_none();
                let total: f32 = raw_local.windows(2).map(|w| seg_len(w[0], w[1])).sum();
                let vs = e.start.map(|v| v.0 as usize);
                let ve = e.end.map(|v| v.0 as usize);
                let same_cluster = match (vs, ve) {
                    (Some(a), Some(b)) => corner_slot[&a] == corner_slot[&b],
                    _ => false,
                };
                let collapse = (same_cluster && total < min_spacing_local)
                    || (closed && total < 4.0 * min_spacing_local);
                if !collapse {
                    let chain =
                        build_chain(&raw_local, closed, (f32::INFINITY, f32::INFINITY));
                    let mut note = |v: Option<usize>, d: f32| {
                        if let Some(vid) = v {
                            let slot = corner_gap.entry(vid).or_insert(f32::INFINITY);
                            *slot = slot.min(d);
                        }
                    };
                    if chain.len() >= 2 {
                        note(vs, seg_len(chain[0], chain[1]));
                        note(ve, seg_len(chain[chain.len() - 2], chain[chain.len() - 1]));
                    }
                }
                pedges.push(PEdge { raw_local, closed, vs, ve, collapse });
            }

            let mut edge_chain: HashMap<u32, Vec<u32>> = HashMap::new();
            for (eid, (e, pe)) in brep.edges.iter().zip(&pedges).enumerate() {
                if pe.collapse {
                    // Offene Mikro-Kante: der geteilte Cluster-Repräsentant
                    // hält die angrenzenden Ringe zusammen. Mikro-Ring: leer —
                    // das Sub-Resolution-Loch verschwindet aus beiden Faces.
                    let idxs: Vec<u32> = match pe.vs {
                        Some(v) if !pe.closed => vec![corner_slot[&v]],
                        _ => Vec::new(),
                    };
                    edge_chain.insert(eid as u32, idxs);
                    continue;
                }
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
                    // SPACE = echter Ketten-Nachbarabstand (nicht h!): space
                    // steuert Stern-Horizont, Grading-Start und den
                    // symmetrischen Konflikt-Test — ein 0.2-dichter Ketten-
                    // punkt mit space=10 „beansprucht" sonst 10 mm Platz und
                    // erklärt ganze Mikro-Patches für redundant (Betreiber-
                    // Screenshot: Grow auf den Wand-Flächen tot).
                    all[ia as usize].space = all[ia as usize].space.min(d);
                    all[ib as usize].space = all[ib as usize].space.min(d);
                };
                for w in idxs.windows(2).map(|w| [w[0], w[1]]).collect::<Vec<_>>() {
                    bound_pair(w[0], w[1], &mut all);
                }
                if closed && idxs.len() >= 2 {
                    bound_pair(idxs[idxs.len() - 1], idxs[0], &mut all);
                }
                edge_chain.insert(eid as u32, idxs);
            }

            // Überdeckungs-Invariante (nur Tests, Ticket 2e7eab20): jedes
            // Ketten-Segment muss von den U-Kreisen seiner Endpunkte
            // überdeckt sein — jede Pipeline-Testfahrt prüft das mit.
            #[cfg(test)]
            for (eid, idxs) in &edge_chain {
                if std::env::var("LICHEN_DEBUG_CHAIN").is_ok() {
                    let e = &brep.edges[*eid as usize];
                    let lens: Vec<String> = idxs
                        .windows(2)
                        .map(|w| format!("{:.3}", seg_len(all[w[0] as usize].p, all[w[1] as usize].p)))
                        .collect();
                    let us: Vec<String> =
                        idxs.iter().map(|&i| format!("{:.3}", all[i as usize].u)).collect();
                    eprintln!(
                        "CHAIN {eid}: n={} closed={} start={:?} end={:?} segs={:?} us={:?}",
                        idxs.len(), e.start.is_none() && e.end.is_none(), e.start, e.end, lens, us
                    );
                }
                for w in idxs.windows(2) {
                    let (a, b) = (&all[w[0] as usize], &all[w[1] as usize]);
                    let d = seg_len(a.p, b.p);
                    assert!(
                        a.u + b.u >= d * 0.999,
                        "U(p)-Überdeckungslücke auf Kante {eid}: d={d:.4} ua={:.4} ub={:.4} bei {:?}",
                        a.u,
                        b.u,
                        a.p
                    );
                }
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
                if std::env::var("LICHEN_DEBUG_FACES").is_ok() {
                    eprintln!(
                        "BREP-FACE fi={fi} {:?} outer.edges={} inner={}",
                        std::mem::discriminant(surface),
                        face.outer.edges.len(),
                        face.inner.len()
                    );
                }
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

            // ── F2a: VERWAISTE Branch-Flächen — der Weave lässt Flächen aus,
            // deren Trim-Nähte er droppt (DL-71 skippt non-Lipschitz-Gruppen;
            // im Betreiber-Demo bleibt so die KUGEL gegen den Gyroid komplett
            // ungewebt und ihre ganze Region ungemesht). Der Träger steht
            // trotzdem im branch_halfspace_map (BoundedCsg = Welt-Frame →
            // placement = IDENTITY in der FaceSurf-Kette); die Rand-Loops
            // entstehen durch Verkettung der angrenzenden Kanten-Ketten
            // (Kanten, deren Punkte alle auf der Surface liegen). Damit
            // wächst und trianguliert die Fläche wie jede gewebte. ──
            let woven: HashSet<u32> =
                brep.faces.iter().map(|f| f.branch_id.as_u32()).collect();
            let hs_map = crate::brep::weaving::branch_halfspace_map(&bodies[bi]);
            let mut orphan_ids: Vec<u32> =
                hs_map.keys().copied().filter(|b| !woven.contains(b)).collect();
            orphan_ids.sort_unstable();
            if std::env::var("LICHEN_DEBUG_FACES").is_ok() {
                eprintln!("ORPHAN candidates: {orphan_ids:?} (woven: {woven:?})");
            }
            for bid in orphan_ids {
                let (hsurf, hxf) = &hs_map[&bid];
                let on_surf = |gidx: u32| -> bool {
                    let ps = crate::world_to_local(hxf, all[gidx as usize].p);
                    crate::csg::sdf_halfspace(hsurf, ps).abs() < (0.02 * h).max(0.05)
                };
                // Angrenzende Kanten: alle Kettenpunkte auf der Surface.
                let mut adj: Vec<u32> = Vec::new();
                for (eid, idxs) in &edge_chain {
                    if idxs.len() >= 2 && idxs.iter().all(|&g| on_surf(g)) {
                        adj.push(*eid);
                    }
                }
                if std::env::var("LICHEN_DEBUG_FACES").is_ok() {
                    let sample: Vec<String> = edge_chain
                        .values()
                        .take(3)
                        .filter_map(|idxs| idxs.first())
                        .map(|&g| {
                            let ps = crate::world_to_local(hxf, all[g as usize].p);
                            format!("{:.3}", crate::csg::sdf_halfspace(hsurf, ps))
                        })
                        .collect();
                    eprintln!("ORPHAN {bid}: adj={} sample-sdf={sample:?}", adj.len());
                }
                adj.sort_unstable();
                // Ketten an geteilten Endpunkten zu Loops verketten (die
                // Loop-Struktur der fehlenden Face gibt es ja nicht).
                let mut rings: Vec<(Vec<u32>, bool)> = Vec::new();
                let mut used: HashSet<u32> = HashSet::new();
                for &e0 in &adj {
                    if used.contains(&e0) {
                        continue;
                    }
                    used.insert(e0);
                    let mut ring: Vec<u32> = edge_chain[&e0].clone();
                    if ring.len() >= 2 && ring.first() == ring.last() {
                        ring.pop(); // geschlossene Kante = fertiger Ring
                    } else {
                        loop {
                            let tail = *ring.last().unwrap();
                            if ring.len() > 1 && tail == ring[0] {
                                ring.pop();
                                break;
                            }
                            let Some(&next) = adj.iter().find(|&&e| {
                                if used.contains(&e) {
                                    return false;
                                }
                                let c = &edge_chain[&e];
                                c.first() == Some(&tail) || c.last() == Some(&tail)
                            }) else {
                                break;
                            };
                            used.insert(next);
                            let mut c = edge_chain[&next].clone();
                            if c.last() == Some(&tail) {
                                c.reverse();
                            }
                            ring.extend(c.into_iter().skip(1));
                        }
                    }
                    if ring.len() >= 3 {
                        rings.push((ring, false));
                    }
                }
                let gf = face_surfs.len() as u32;
                // Pro-Fläche-Normale = ∇(Surface-SDF), Surface-Frame → Welt.
                let normal_at = |p_world: [f32; 3]| -> [f32; 3] {
                    let ps = crate::world_to_local(hxf, p_world);
                    let e = 1e-3f32;
                    let sd = |q: [f32; 3]| crate::csg::sdf_halfspace(hsurf, q);
                    let g = [
                        sd([ps[0] + e, ps[1], ps[2]]) - sd([ps[0] - e, ps[1], ps[2]]),
                        sd([ps[0], ps[1] + e, ps[2]]) - sd([ps[0], ps[1] - e, ps[2]]),
                        sd([ps[0], ps[1], ps[2] + e]) - sd([ps[0], ps[1], ps[2] - e]),
                    ];
                    lichen::unit(crate::quat_rotate(hxf.rotation, g))
                };
                // Kettenlos (die Nähte wurden im Weave gedroppt): FELD-
                // STREUUNG — Gitter über die Body-AABB, Newton-Projektion auf
                // die Surface, behalten wo der Punkt AUF dem Body liegt
                // (|f|≈0) — Surface-Stücke im Material (f<0) oder außerhalb
                // (f>0) fallen raus. Jeder Gyroid-Zwischenraum bekommt so
                // seine eigenen Seeds (Patches sind nicht zusammenhängend).
                let mut scatter: Vec<([f32; 3], [f32; 3])> = Vec::new();
                if rings.is_empty() {
                    // Body-AABB; Gyroid-Leafs haben EMPTY-AABB (unendlich) und
                    // vergiften die Intersection — dann Punktwolken-AABB der
                    // bereits gesammelten Ecken/Ketten + 2h Marge als Hülle.
                    let mut bb = bodies[bi].aabb();
                    if !(bb.max[0] > bb.min[0]) {
                        bb = crate::csg::Aabb::EMPTY;
                        for lp in &all {
                            for a in 0..3 {
                                bb.min[a] = bb.min[a].min(lp.p[a] - 2.0 * h);
                                bb.max[a] = bb.max[a].max(lp.p[a] + 2.0 * h);
                            }
                        }
                    }
                    let ext = (bb.max[0] - bb.min[0])
                        .max(bb.max[1] - bb.min[1])
                        .max(bb.max[2] - bb.min[2]);
                    if ext > 0.0 && ext.is_finite() {
                        let step = (0.4 * h).max(ext / 64.0);
                        let (mut rej_newton, mut rej_body, mut rej_dedup, mut tried) =
                            (0usize, 0usize, 0usize, 0usize);
                        let n = [
                            ((bb.max[0] - bb.min[0]) / step).ceil() as i32 + 1,
                            ((bb.max[1] - bb.min[1]) / step).ceil() as i32 + 1,
                            ((bb.max[2] - bb.min[2]) / step).ceil() as i32 + 1,
                        ];
                        'grid: for ix in 0..n[0] {
                            for iy in 0..n[1] {
                                for iz in 0..n[2] {
                                    let mut p = [
                                        bb.min[0] + ix as f32 * step,
                                        bb.min[1] + iy as f32 * step,
                                        bb.min[2] + iz as f32 * step,
                                    ];
                                    // Newton auf die Trägerfläche.
                                    let mut d = f32::MAX;
                                    for _ in 0..8 {
                                        let ps = crate::world_to_local(hxf, p);
                                        d = crate::csg::sdf_halfspace(hsurf, ps);
                                        if d.abs() < 1e-3 {
                                            break;
                                        }
                                        let nrm = normal_at(p);
                                        p = [p[0] - d * nrm[0], p[1] - d * nrm[1], p[2] - d * nrm[2]];
                                    }
                                    tried += 1;
                                    if d.abs() > 0.05 {
                                        rej_newton += 1;
                                        continue;
                                    }
                                    // Nur AUF dem Body (aktive Boundary-Region).
                                    if fg(p).0.abs() > 0.05 * h {
                                        rej_body += 1;
                                        continue;
                                    }
                                    // Dedup gegen bereits gestreute Seeds.
                                    let min_d2 = (0.7 * h) * (0.7 * h);
                                    if scatter.iter().any(|(q, _)| {
                                        let dx = q[0] - p[0];
                                        let dy = q[1] - p[1];
                                        let dz = q[2] - p[2];
                                        dx * dx + dy * dy + dz * dz < min_d2
                                    }) {
                                        rej_dedup += 1;
                                        continue;
                                    }
                                    scatter.push((p, normal_at(p)));
                                    // Existenz-Probe genügt — die eigentlichen
                                    // Seeds streut der Wachstums-Pass für ALLE
                                    // Flächen ohne Rand-Seeds einheitlich.
                                    break 'grid;
                                }
                            }
                        }
                        if std::env::var("LICHEN_DEBUG_FACES").is_ok() {
                            eprintln!(
                                "ORPHAN {bid}: tried={tried} rej_newton={rej_newton} rej_body={rej_body} rej_dedup={rej_dedup} ext={ext} step={step}"
                            );
                        }
                    }
                }
                if std::env::var("LICHEN_DEBUG_FACES").is_ok() {
                    eprintln!("ORPHAN {bid}: rings={} scatter={}", rings.len(), scatter.len());
                }
                if rings.is_empty() && scatter.is_empty() {
                    continue; // kein Oberflächen-Anteil am Body
                }
                let mut seen: HashSet<u32> = HashSet::new();
                for (ring, _) in &rings {
                    for &gidx in ring {
                        if seen.insert(gidx) {
                            let nw = normal_at(all[gidx as usize].p);
                            point_faces[gidx as usize].push((gf, nw));
                        }
                    }
                }
                face_surfs.push(FaceSurf {
                    surface: hsurf.clone(),
                    transform: *hxf,
                    placement: Transform::IDENTITY,
                });
                face_loops.push(rings);
            }
        }

        // Welt→Fläche-Klassifikator (harte Kanten-Wand): die BRep-Fläche mit
        // kleinstem |Flächen-SDF| am Punkt; `None`, wenn zu weit von allen
        // Flächen (Blend-Region) — dorthin wächst nichts.
        // SKALEN-KORREKTUR (Betreiber-Bild 3, „Spalt in der Mini-Kugel"):
        // sdf_halfspace misst im LOKALEN Flächen-Frame — eine Fläche mit
        // placement-/transform-Skala 0.02 (Mini-Kugel) liefert 50× auf-
        // geblasene Distanzen. Der argmin über verschieden skalierte
        // Flächen war damit unfair, und alle Distanz-Schwellen (0.5·h,
        // on_face-Wand) wirkten pro Fläche verschieden. Immer auf WELT-mm
        // zurückskalieren.
        let face_scale = |fs: &FaceSurf| -> f32 {
            (fs.placement.scale[0] * fs.transform.scale[0]).abs().max(1e-6)
        };
        let face_of = |p_world: [f32; 3]| -> Option<u32> {
            let mut best = u32::MAX;
            let mut bestd = f32::MAX;
            for (gf, fs) in face_surfs.iter().enumerate() {
                let p_local = crate::world_to_local(&fs.placement, p_world);
                let p_surf = crate::world_to_local(&fs.transform, p_local);
                let d = crate::csg::sdf_halfspace(&fs.surface, p_surf).abs() * face_scale(fs);
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
        // PERF: Kanten-Segmente in einen Spatial-Hash (Zelle = h). Der
        // lineare Scan über ALLE Segmente lief pro Wachstums-Kandidat
        // mehrfach (brep_dist + Ketten-Vorhang) — bei feinen Settings
        // zehntausende Segmente × hunderttausende Aufrufe = der größte
        // Einzelfresser (Betreiber: „andere Verfahren schaffen Millionen
        // Dreiecke, wir nicht mal tausend").
        let seg_cell = h.max(1e-3);
        let seg_key = |p: [f32; 3]| -> (i32, i32, i32) {
            (
                (p[0] / seg_cell).floor() as i32,
                (p[1] / seg_cell).floor() as i32,
                (p[2] / seg_cell).floor() as i32,
            )
        };
        let mut seg_grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        for (si, &(a, b)) in edge_segs.iter().enumerate() {
            let lo = [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])];
            let hi = [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])];
            let (k0, k1) = (seg_key(lo), seg_key(hi));
            for x in k0.0..=k1.0 {
                for y in k0.1..=k1.1 {
                    for z in k0.2..=k1.2 {
                        seg_grid.entry((x, y, z)).or_default().push(si as u32);
                    }
                }
            }
        }
        let seg_dist_pt = |p: [f32; 3], si: u32| -> f32 {
            let (a, b) = edge_segs[si as usize];
            let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
            let denom = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
            let t = if denom > 1e-20 {
                ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
        };
        // Kleinster Segment-Abstand, GEDECKELT auf h: alle Nutzer (U-Kreis-
        // Deckel, Kanten-Nähe-Checks) interessieren sich nur für Distanzen
        // unter h — ein ±1-Zellring (Zelle = h) deckt das ab.
        let brep_dist = |p: [f32; 3]| -> f32 {
            let (kx, ky, kz) = seg_key(p);
            let mut best = h;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let Some(v) = seg_grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                            continue;
                        };
                        for &si in v {
                            let d = seg_dist_pt(p, si);
                            if d < best {
                                best = d;
                            }
                        }
                    }
                }
            }
            best
        };

        // ── FELD-STREUUNG für Flächen ohne Rand-Seeds (Betreiber-Punkt:
        // der Basis-Algorithmus muss OHNE Fallbacks und ohne Rand-Topologie
        // funktionieren): geschlossene Träger (freie Kugel/Torus/Gyroid)
        // und loop-lose Orphan-Flächen bekommen Seeds per Gitter über die
        // Szenen-Hülle + Newton-Projektion auf die Trägerfläche. Behalten
        // wird nur, was AUF dem Body liegt (|f|≈0) UND in der eigenen
        // face_of-Region. Newton skalen-korrekt: der Surface-SDF lebt im
        // lokalen Frame, der Welt-Schritt ist um die Placement-/Transform-
        // Skala zu strecken (Mini-Kugel: scale 0.02!). ──
        let scene_bb = {
            let mut bb = crate::csg::Aabb::EMPTY;
            for &bi in &active {
                let b = bodies[bi].aabb();
                if b.max[0] > b.min[0] {
                    for a in 0..3 {
                        bb.min[a] = bb.min[a].min(b.min[a]);
                        bb.max[a] = bb.max[a].max(b.max[a]);
                    }
                }
            }
            // Gyroid-Leafs haben EMPTY-AABB (unendlich) — Punkt-Hülle als
            // Ergänzung, damit die Hülle nie leer ist.
            for lp in &all {
                for a in 0..3 {
                    bb.min[a] = bb.min[a].min(lp.p[a] - 2.0 * h);
                    bb.max[a] = bb.max[a].max(lp.p[a] + 2.0 * h);
                }
            }
            bb
        };
        let scatter_face = |gf: u32| -> Vec<([f32; 3], [f32; 3])> {
            let fs = &face_surfs[gf as usize];
            let total_scale =
                (fs.placement.scale[0] * fs.transform.scale[0]).abs().max(1e-6);
            let surf_normal = |p_world: [f32; 3]| -> [f32; 3] {
                let pl = crate::world_to_local(&fs.placement, p_world);
                let ps = crate::world_to_local(&fs.transform, pl);
                let e = 1e-3f32;
                let sd = |q: [f32; 3]| crate::csg::sdf_halfspace(&fs.surface, q);
                let g = [
                    sd([ps[0] + e, ps[1], ps[2]]) - sd([ps[0] - e, ps[1], ps[2]]),
                    sd([ps[0], ps[1] + e, ps[2]]) - sd([ps[0], ps[1], ps[2]]),
                    sd([ps[0], ps[1], ps[2] + e]) - sd([ps[0], ps[1], ps[2] - e]),
                ];
                let gl = crate::quat_rotate(fs.transform.rotation, g);
                lichen::unit(crate::quat_rotate(fs.placement.rotation, gl))
            };
            let mut out: Vec<([f32; 3], [f32; 3])> = Vec::new();
            let bb = scene_bb;
            let ext = (bb.max[0] - bb.min[0])
                .max(bb.max[1] - bb.min[1])
                .max(bb.max[2] - bb.min[2]);
            if !(ext > 0.0) || !ext.is_finite() {
                return out;
            }
            let step = (0.4 * h).max(ext / 64.0);
            let n = [
                ((bb.max[0] - bb.min[0]) / step).ceil() as i32 + 1,
                ((bb.max[1] - bb.min[1]) / step).ceil() as i32 + 1,
                ((bb.max[2] - bb.min[2]) / step).ceil() as i32 + 1,
            ];
            'grid: for ix in 0..n[0] {
                for iy in 0..n[1] {
                    for iz in 0..n[2] {
                        let mut p = [
                            bb.min[0] + ix as f32 * step,
                            bb.min[1] + iy as f32 * step,
                            bb.min[2] + iz as f32 * step,
                        ];
                        let mut d = f32::MAX;
                        for _ in 0..12 {
                            let pl = crate::world_to_local(&fs.placement, p);
                            let ps = crate::world_to_local(&fs.transform, pl);
                            d = crate::csg::sdf_halfspace(&fs.surface, ps);
                            if d.abs() * total_scale < 1e-3 {
                                break;
                            }
                            let nrm = surf_normal(p);
                            let w = d * total_scale;
                            p = [p[0] - w * nrm[0], p[1] - w * nrm[1], p[2] - w * nrm[2]];
                        }
                        if d.abs() * total_scale > 0.05 {
                            continue;
                        }
                        if fg(p).0.abs() > 0.05 * h {
                            continue;
                        }
                        if face_of(p) != Some(gf) {
                            continue;
                        }
                        let min_d2 = (0.7 * h.min(ext / 8.0)) * (0.7 * h.min(ext / 8.0));
                        if out.iter().any(|(q, _)| {
                            let dx = q[0] - p[0];
                            let dy = q[1] - p[1];
                            let dz = q[2] - p[2];
                            dx * dx + dy * dy + dz * dz < min_d2
                        }) {
                            continue;
                        }
                        out.push((p, surf_normal(p)));
                        if out.len() >= 4000 {
                            break 'grid;
                        }
                    }
                }
            }
            out
        };

        // Phasen-Stoppuhren nur NATIV: std::time::Instant PANICT auf
        // wasm32 („time not implemented") — die 541er-Instrumentierung
        // hat damit den Browser-Mesher getötet (Betreiber-Konsole).
        #[cfg(not(target_arch = "wasm32"))]
        let t_setup = std::time::Instant::now();
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
                            // Das ECHTE Rand-Spacing mitgeben (Ketten können
                            // weit unter h liegen): das Wachstum gradet davon
                            // ins Flächen-Innere (≤×1.5 pro Schritt) — sonst
                            // kollidiert jeder Kandidat eines dichten Rands
                            // mit den Ketten und die Fläche wächst gar nicht
                            // (Demo-Mikro-Gyroid: 0 Innenpunkte).
                            space: src.space.min(h),
                            flag: src.flag,
                        });
                    }
                }
                // Flächen ohne Rand-Seeds (geschlossene Träger, loop-lose
                // Orphans): Feld-Streuung liefert die Start-Punkte.
                if bseeds.is_empty() {
                    for (p, nrm) in scatter_face(gf) {
                        let idx = all.len() as u32;
                        let (d, _) = fg(p);
                        all.push(LPoint { p, sdf: d, n: nrm, u: h, space: h, flag: 1.0 });
                        point_faces.push(vec![(gf, nrm)]);
                        local_pos.push([0.0; 3]);
                        face_interior[gf as usize].push(idx);
                        bseeds.push(all[idx as usize]);
                    }
                }
                if bseeds.is_empty() {
                    continue;
                }
                // TOPOLOGISCHE Kanten-Wand (Betreiber-Screenshot 2026-07-11,
                // „Grow auf den flachen Seiten tot"): der frühere Distanz-
                // argmin ist an TANGENTIALEN Verschneidungen (Gyroid schneidet
                // Box-Ebene fast parallel) ein Münzwurf — beide Flächen sind
                // dort über mm-Streifen gleich nah, fast jeder Kandidat wurde
                // gefressen. Die WÄNDE sind aber die KETTEN selbst: ein
                // Schritt ist gültig, wenn (a) das Ziel auf der EIGENEN
                // Trägerfläche liegt und (b) der Schritt p→cp keine Ketten-
                // Kante kreuzt (2D-Test im Tangentialrahmen). Eine bloße
                // Mit-Minimal-Toleranz wurde zuvor vermessen und verworfen
                // (Demo 148→229 + non-manifold) — sie hatte keinen Ketten-
                // Vorhang.
                let own_dist = |q: [f32; 3]| -> f32 {
                    let fs = &face_surfs[gf as usize];
                    let pl = crate::world_to_local(&fs.placement, q);
                    let ps = crate::world_to_local(&fs.transform, pl);
                    // In Welt-mm (Skalen-Korrektur, s. face_of).
                    crate::csg::sdf_halfspace(&fs.surface, ps).abs() * face_scale(fs)
                };
                let on_face = |p0: [f32; 3], p1: [f32; 3], rr: f32| -> bool {
                    // (a) Ziel auf der eigenen Trägerfläche. Floor 2e-3·h
                    // statt 0.02·h: der alte Floor (0.2 mm bei h=10) machte
                    // die Wand für Sub-h-Features PORÖS — die Gyroid-Wand
                    // wuchs bis 0.2 mm neben ihren Träger, d.h. quer über
                    // das Band der Mini-Kugel (Wand–Kugel-Abstand dort
                    // 0.05–0.15 mm); ihre Übertreter blockierten die echte
                    // Kugel-Besiedlung als xgrid-Konflikte und fehlten im
                    // Kugel-Chart → leeres Band, lange Brücken-Sehnen
                    // (Betreiber-Bild „Spalt in der Kugel").
                    if own_dist(p1) > (0.15 * rr).max(2e-3 * h) {
                        return false;
                    }

                    // (b) Ketten-Vorhang: Schritt kreuzt keine Kanten-Kette.
                    let n = {
                        let d = [
                            (p0[0] + p1[0]) * 0.5,
                            (p0[1] + p1[1]) * 0.5,
                            (p0[2] + p1[2]) * 0.5,
                        ];
                        // robust_grad: der Dual-Gradient ist exakt auf
                        // planaren Flächen null (bekannter Wurzel-Bug) —
                        // ein Müll-Rahmen macht den Kreuzungstest zufällig.
                        lichen::unit(lichen::robust_grad(d, &fg))
                    };
                    let seed = if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
                    let dn = seed[0] * n[0] + seed[1] * n[1] + seed[2] * n[2];
                    let e1 = lichen::unit([
                        seed[0] - dn * n[0],
                        seed[1] - dn * n[1],
                        seed[2] - dn * n[2],
                    ]);
                    let e2 = [
                        n[1] * e1[2] - n[2] * e1[1],
                        n[2] * e1[0] - n[0] * e1[2],
                        n[0] * e1[1] - n[1] * e1[0],
                    ];
                    let to2 = |q: [f32; 3]| -> [f64; 2] {
                        let d = [q[0] - p0[0], q[1] - p0[1], q[2] - p0[2]];
                        [
                            (d[0] * e1[0] + d[1] * e1[1] + d[2] * e1[2]) as f64,
                            (d[0] * e2[0] + d[1] * e2[1] + d[2] * e2[2]) as f64,
                        ]
                    };
                    let (a2, b2) = (to2(p0), to2(p1));
                    let step2 = ((p1[0] - p0[0]).powi(2)
                        + (p1[1] - p0[1]).powi(2)
                        + (p1[2] - p0[2]).powi(2))
                    .sqrt();
                    // Segment-Spatial-Hash statt Voll-Scan (PERF): der
                    // Schritt ist ≤ h lang, ±1 Zellring (Zelle = h) genügt.
                    let (kx, ky, kz) = seg_key(p0);
                    for dx in -1..=1 {
                        for dy in -1..=1 {
                            for dz in -1..=1 {
                                let Some(v) = seg_grid.get(&(kx + dx, ky + dy, kz + dz)) else {
                                    continue;
                                };
                                for &si in v {
                                    let (sa, sb) = edge_segs[si as usize];
                                    let m = [
                                        (sa[0] + sb[0]) * 0.5,
                                        (sa[1] + sb[1]) * 0.5,
                                        (sa[2] + sb[2]) * 0.5,
                                    ];
                                    let dm = ((m[0] - p0[0]).powi(2)
                                        + (m[1] - p0[1]).powi(2)
                                        + (m[2] - p0[2]).powi(2))
                                    .sqrt();
                                    let seg_half = ((sb[0] - sa[0]).powi(2)
                                        + (sb[1] - sa[1]).powi(2)
                                        + (sb[2] - sa[2]).powi(2))
                                    .sqrt()
                                        * 0.5;
                                    if dm > step2 + seg_half + 1e-3 {
                                        continue;
                                    }
                                    if crate::lichen_tri::segments_cross(a2, b2, to2(sa), to2(sb)) {
                                        return false;
                                    }
                                }
                            }
                        }
                    }
                    true
                };
                // Krümmung von der TRÄGERFLÄCHE dieser Fläche: Normalenfeld
                // aus dem Surface-SDF (glatt, unendlich — misst nie einen
                // B-Rep-Kanten-Knick und rauscht nicht wie Klein-Fenster-FD
                // am Body-Feld).
                let surf_gradn = |q: [f32; 3]| -> [f32; 3] {
                    let fs = &face_surfs[gf as usize];
                    let pl = crate::world_to_local(&fs.placement, q);
                    let ps = crate::world_to_local(&fs.transform, pl);
                    let e = 1e-3f32;
                    let sd = |w: [f32; 3]| crate::csg::sdf_halfspace(&fs.surface, w);
                    let g = [
                        sd([ps[0] + e, ps[1], ps[2]]) - sd([ps[0] - e, ps[1], ps[2]]),
                        sd([ps[0], ps[1] + e, ps[2]]) - sd([ps[0], ps[1] - e, ps[2]]),
                        sd([ps[0], ps[1], ps[2] + e]) - sd([ps[0], ps[1], ps[2] - e]),
                    ];
                    let gl = crate::quat_rotate(fs.transform.rotation, g);
                    lichen::unit(crate::quat_rotate(fs.placement.rotation, gl))
                };
                // Krümmung ZWEISTUFIG: erst mit dem globalen h grob schätzen,
                // daraus einen Radius ableiten, dann mit einem an DIESEN
                // Radius gekoppelten FD-Fenster nachmessen. Ein Fixfenster von
                // 0,1·h ist auf feinen Features (Gyroid-Wand 1 mm bei h = 10)
                // blind — es glättet die Krümmung weg, das Wachstum bleibt
                // grob, und die Normalen benachbarter Punkte drehen sich dann
                // um 90° statt um den Krümmungswinkel.
                let surf_curv = |q: [f32; 3]| -> (f32, [f32; 3]) {
                    let n = surf_gradn(q);
                    let (k0, _) = lichen::curvature(q, n, h, &surf_gradn);
                    let scale = if k0 > 1e-6 {
                        (opts.curv_angle_deg.to_radians() / k0).clamp(opts.min_edge_spacing.max(1e-4), h)
                    } else {
                        h
                    };
                    lichen::curvature(q, n, scale, &surf_gradn)
                };
                #[cfg(not(target_arch = "wasm32"))]
                let t_g = std::time::Instant::now();
                let grown = lichen::grow_surface(
                    &bseeds, &all, h, &opts, &fg, &brep_dist, &on_face, &surf_curv, size,
                );
                #[cfg(not(target_arch = "wasm32"))]
                if std::env::var("LICHEN_PERF").is_ok() {
                    eprintln!("PERF grow gf={gf}: {:?} ({} Punkte)", t_g.elapsed(), grown.len());
                }
                if std::env::var("LICHEN_DEBUG_GROW").is_ok() {
                    let bad = grown
                        .iter()
                        .filter(|g| g.n == [0.0, 0.0, 1.0] || g.n == [0.0, 0.0, -1.0])
                        .count();
                    let tiny = grown.iter().filter(|g| g.space < 0.3 * h).count();
                    eprintln!(
                        "GROW gf={gf}: n={} default-n={bad} tiny-space={tiny} seeds={}",
                        grown.len(), bseeds.len()
                    );
                }
                for g in grown {
                    let idx = all.len() as u32;
                    all.push(g);
                    point_faces.push(vec![(gf, g.n)]);
                    local_pos.push([0.0; 3]);
                    face_interior[gf as usize].push(idx);
                }
            }

            // ── FLÄCHEN-EINFÄRBUNG (Betreiber-Idee 2026-07-12) ──
            // Jeder gewachsene Punkt trägt bisher GENAU EINE Flächen-ID (die,
            // auf der er gewachsen ist). An einer TANGENTIALEN Verschneidung
            // liegen aber zwei Flächen über einen breiten Streifen fast
            // aufeinander — dort ist ein Punkt de facto BEIDEN Flächen nahe,
            // und genau dort scheitert jeder normalen-basierte Guard (die
            // Normalen sind sich dort ÄHNLICH, nicht antiparallel).
            //
            // Also: jeder Punkt bekommt die MENGE aller Flächen, auf denen er
            // (in Welt-mm) innerhalb einer Spacing-Toleranz liegt. Punkte im
            // Naht-Band tragen zwei IDs und dürfen legitim vermitteln; Punkte
            // eindeutig auf einer Fläche bekommen genau eine — und ein Dreieck
            // ist nur gültig, wenn seine drei Ecken eine GEMEINSAME Fläche
            // haben (`face_guard`). Das ist eine TOPOLOGISCHE Bedingung, die
            // der Geometrie nicht ansieht, wie nah sich zwei Blätter kommen.
            if opts.face_guard {
                let mut extra = 0usize;
                for i in 0..all.len() {
                    if all[i].flag > 1.5 {
                        continue; // Ketten/Ecken tragen ihre Flächen schon
                    }
                    let p = all[i].p;
                    let band: f32 = std::env::var("LICHEN_FACE_BAND")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0.5);
                    let tang_deg: f32 = std::env::var("LICHEN_FACE_TANGENT_DEG")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(25.0);
                    let tang_cos = tang_deg.to_radians().cos();
                    let tol = (band * all[i].space).max(1e-4);
                    for (gf, fs) in face_surfs.iter().enumerate() {
                        let gf = gf as u32;
                        if point_faces[i].iter().any(|&(g, _)| g == gf) {
                            continue;
                        }
                        let pl = crate::world_to_local(&fs.placement, p);
                        let ps = crate::world_to_local(&fs.transform, pl);
                        let d = crate::csg::sdf_halfspace(&fs.surface, ps).abs() * face_scale(fs);
                        if d <= tol {
                            let e = 1e-3f32;
                            let sd = |q: [f32; 3]| crate::csg::sdf_halfspace(&fs.surface, q);
                            let g = [
                                sd([ps[0] + e, ps[1], ps[2]]) - sd([ps[0] - e, ps[1], ps[2]]),
                                sd([ps[0], ps[1] + e, ps[2]]) - sd([ps[0], ps[1] - e, ps[2]]),
                                sd([ps[0], ps[1], ps[2] + e]) - sd([ps[0], ps[1], ps[2] - e]),
                            ];
                            let gl = crate::quat_rotate(fs.transform.rotation, g);
                            let nw = lichen::unit(crate::quat_rotate(fs.placement.rotation, gl));
                            // REGIONEN-KORREKTHEIT — die Träger sind UNENDLICH!
                            // Nähe zum TRÄGER heißt NICHT Zugehörigkeit zur
                            // FLÄCHE: ein kantennaher Punkt der Box-Deckfläche
                            // ist auch dem Träger der SEITENfläche nah — dort
                            // steht dieser aber QUER (90°). Eine Zweitzuordnung
                            // ist nur sinnvoll, wo die beiden Träger am Punkt
                            // fast PARALLEL sind: genau das ist die TANGENTIALE
                            // Verschneidung, für die dieser Guard existiert
                            // (und genau die, die kein normalen-basierter Test
                            // sehen kann). An einer scharfen Kante trennt die
                            // Kette ohnehin sauber.
                            let n_own = all[i].n;
                            let cos = (nw[0] * n_own[0] + nw[1] * n_own[1] + nw[2] * n_own[2]).abs();
                            if cos < tang_cos {
                                continue;
                            }
                            point_faces[i].push((gf, nw));
                            extra += 1;
                        }
                    }
                }
                if std::env::var("LICHEN_DEBUG_FACES").is_ok() {
                    eprintln!("FLÄCHEN-EINFÄRBUNG: {extra} Mehrfach-Zuordnungen (Naht-Bänder)");
                }
            }
        }

        // ── SIZING-FIELD (zweiter Durchgang) ──
        // Die Wolke aus diesem groben Lauf dient als STÜTZSTELLEN-Menge: an
        // ihr wird die lfs mit einem FEINEN Marsch gemessen (der grobe kann
        // kleine Spalte gar nicht sehen — er meldet seinen ersten Schritt),
        // das Feld Lipschitz-geglättet und die GESAMTE Abtastung damit
        // wiederholt. Ohne die globale Glättung schadet jede Verfeinerung mehr,
        // als sie nützt (zweimal vermessen: feiner Marsch → Demo 295→619,
        // Keil-Spacing aus der Wolke → Box 0→3).
        if opts.sizing_field && size.is_none() && opts.grow {
            let eps = if opts.lfs_factor > 0.0 { opts.lfs_factor } else { 0.5 };
            let cap_lfs = h / eps;
            let mut pts: Vec<[f32; 3]> = Vec::with_capacity(all.len());
            let mut raw: Vec<f32> = Vec::with_capacity(all.len());
            for lp in &all {
                let n = lichen::unit(lichen::robust_grad(lp.p, &fg));
                // FEINER Startschritt, an das lokale Spacing gekoppelt: nur so
                // wird ein Spalt gefunden, der kleiner ist als das Spacing.
                let t0 = (0.05 * lp.space).max(1e-5);
                let lfs = lichen::reach_inner(lp.p, n, cap_lfs, t0, &fg)
                    .min(lichen::reach_outer_flat(lp.p, n, cap_lfs, t0, &fg));
                let mut r = h.min(lp.space.max(1e-4));
                if lfs < cap_lfs {
                    r = r.min(eps * lfs);
                }
                pts.push(lp.p);
                raw.push(r);
            }
            let k_lip: f32 = std::env::var("LICHEN_LIPSCHITZ")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.3);
            let floor = opts.min_edge_spacing.max(1e-4).min(h);
            let sf = lichen::SizeField::build(pts, raw, k_lip, h, floor);
            if std::env::var("LICHEN_DEBUG_SIZING").is_ok() {
                eprintln!("SIZING-FIELD: {} Stützstellen, K={k_lip}, floor={floor}", all.len());
            }
            return self.lichen_mesh_sized(opts, body_filter, Some(&sf));
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
        #[cfg(not(target_arch = "wasm32"))]
        if std::env::var("LICHEN_PERF").is_ok() {
            eprintln!("PERF setup+grow gesamt: {:?} ({} Punkte)", t_setup.elapsed(), all.len());
        }
        // Diagnose fürs Overlay (Chart-Modus): weak-Tris der Fallback-Flächen;
        // die finalen offenen Kanten werden nach Dedup/Orientierung unten
        // berechnet (deckungsgleich mit dem Watertight-Zähler).
        let mut diag_weak: Vec<[u32; 3]> = Vec::new();
        let tris = if opts.triangulate && all.len() >= 3 {
            let mut t: Vec<[u32; 3]> = Vec::new();
            // Allgemeiner Pfad (Stern-Konsistenz, chartlos): EIN Code für
            // plan + gekrümmt; Diagnose-first (Löcher gezählt, nie übermalt).
            if opts.general_tri {
                let positions: Vec<[f32; 3]> = all.iter().map(|lp| lp.p).collect();
                let spacings: Vec<f32> = all.iter().map(|lp| lp.space).collect();
                // ── GLOBALER WALK: ein Netz über ALLE Punkte (s. lichen_tri::
                // triangulate_global_walk). Schließt die Naht-Schlitze an
                // gedroppten B-Rep-Kanten, die per-Fläche prinzipiell offen
                // bleiben müssen. ──
                if opts.global_walk {
                    let global_chain_edges: HashSet<(u32, u32)> = face_loops
                        .iter()
                        .flat_map(|rings| rings.iter())
                        .flat_map(|(r, _)| {
                            (0..r.len()).map(move |k| {
                                let (a, b) = (r[k], r[(k + 1) % r.len()]);
                                (a.min(b), a.max(b))
                            })
                        })
                        .filter(|&(a, b)| a != b)
                        .collect();
                    let members: Vec<u32> = (0..all.len() as u32).collect();
                    let nmap: HashMap<u32, [f32; 3]> =
                        members.iter().map(|&i| (i, all[i as usize].n)).collect();
                    // Normalen-MENGE je Punkt (eine pro angrenzender Fläche);
                    // Kantenpunkte tragen zwei, Ecken ≥3 — die Basis des
                    // Normalen-Guards (Kanten-Sonderbehandlung).
                    let normal_sets: Vec<Vec<[f32; 3]>> = point_faces
                        .iter()
                        .enumerate()
                        .map(|(i, pf)| {
                            if pf.is_empty() {
                                vec![all[i].n]
                            } else {
                                pf.iter().map(|&(_, n)| n).collect()
                            }
                        })
                        .collect();
                    // FLÄCHEN-MENGEN je Punkt (Betreiber-Idee): Grundlage des
                    // topologischen Flächen-Guards im Walk — aber auf REGIONEN
                    // statt Flächen.
                    //
                    // REGIONEN-VERSCHMELZUNG: der Weave droppt Trim-Nähte
                    // (non-Lipschitz, Gyroid). Zwei Flächen, deren Punkte sich
                    // berühren, die aber KEINE gemeinsame Kette haben, sind
                    // durch nichts getrennt — die B-Rep ist dort unvollständig.
                    // Ein Flächen-Guard, der sie trennt, reißt genau den Naht-
                    // Schlitz auf, den der globale Walk beseitigen sollte.
                    // Also: solche Flächen werden per Union-Find zu EINER
                    // Region verschmolzen. Flächen mit ordentlicher Kette
                    // bleiben getrennt — dort vermitteln die Ketten-Punkte
                    // (die ohnehin beide IDs tragen).
                    let face_sets: Option<Vec<Vec<u32>>> = if opts.face_guard {
                        let nf = face_surfs.len();
                        let mut parent: Vec<u32> = (0..nf as u32).collect();
                        fn find(par: &mut Vec<u32>, x: u32) -> u32 {
                            if par[x as usize] == x {
                                return x;
                            }
                            let r = find(par, par[x as usize]);
                            par[x as usize] = r;
                            r
                        }
                        // Ketten-Punkte je Fläche (aus den Loops).
                        let chain_of: Vec<HashSet<u32>> = face_loops
                            .iter()
                            .map(|rings| rings.iter().flat_map(|(r, _)| r.iter().copied()).collect())
                            .collect();
                        // Berührende Flächen-Paare aus der Punktwolke.
                        let cell = h.max(1e-3);
                        let key = |p: [f32; 3]| {
                            (
                                (p[0] / cell).floor() as i32,
                                (p[1] / cell).floor() as i32,
                                (p[2] / cell).floor() as i32,
                            )
                        };
                        let mut grid: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
                        for gf in 0..nf {
                            for &gi in &face_interior[gf] {
                                grid.entry(key(all[gi as usize].p)).or_default().push(gi);
                            }
                        }
                        let face_of_pt = |gi: u32| -> Option<u32> {
                            point_faces[gi as usize].first().map(|&(g, _)| g)
                        };
                        let mut merged = 0usize;
                        for gf in 0..nf {
                            for &gi in &face_interior[gf] {
                                let p = all[gi as usize].p;
                                let r = 1.5 * all[gi as usize].space;
                                let (kx, ky, kz) = key(p);
                                for dx in -1..=1 {
                                    for dy in -1..=1 {
                                        for dz in -1..=1 {
                                            let Some(v) = grid.get(&(kx + dx, ky + dy, kz + dz))
                                            else {
                                                continue;
                                            };
                                            for &gj in v {
                                                let Some(g2) = face_of_pt(gj) else { continue };
                                                if g2 as usize == gf {
                                                    continue;
                                                }
                                                let q = all[gj as usize].p;
                                                let d = ((p[0] - q[0]).powi(2)
                                                    + (p[1] - q[1]).powi(2)
                                                    + (p[2] - q[2]).powi(2))
                                                .sqrt();
                                                if d > r {
                                                    continue;
                                                }
                                                // Gemeinsame Kette? Dann ist die
                                                // Trennung echt.
                                                let a = &chain_of[gf];
                                                let b = &chain_of[g2 as usize];
                                                if a.iter().any(|x| b.contains(x)) {
                                                    continue;
                                                }
                                                let (ra, rb) =
                                                    (find(&mut parent, gf as u32), find(&mut parent, g2));
                                                if ra != rb {
                                                    parent[rb as usize] = ra;
                                                    merged += 1;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if std::env::var("LICHEN_DEBUG_FACES").is_ok() {
                            eprintln!(
                                "REGIONEN: {merged} Verschmelzungen (Flächen ohne gemeinsame Kette)"
                            );
                        }
                        Some(
                            point_faces
                                .iter()
                                .map(|pf| {
                                    let mut v: Vec<u32> = pf
                                        .iter()
                                        .map(|&(g, _)| find(&mut parent, g))
                                        .collect();
                                    v.sort_unstable();
                                    v.dedup();
                                    v
                                })
                                .collect(),
                        )
                    } else {
                        None
                    };
                    let res = crate::lichen_tri::triangulate_global_walk(
                        &positions,
                        &nmap,
                        &normal_sets,
                        face_sets.as_deref(),
                        &spacings,
                        &members,
                        &global_chain_edges,
                        opts.curv_angle_deg.to_radians(),
                        &opts.accept,
                        &fg,
                    );
                    let mut t = res.tris;
                    lichen::orient_consistent(&all, &mut t);
                    if res.hole_edges + res.nonmanifold_edges > 0 {
                        eprintln!(
                            "lichen global-walk: {} Loch-Kanten, {} non-manifold (Diagnose)",
                            res.hole_edges, res.nonmanifold_edges
                        );
                    }
                    return lichen::pack_with_diag(
                        &all,
                        &t,
                        &point_faces,
                        &res.open_edges,
                        &[],
                    );
                }
                // Globale Ketten-Kanten ALLER Flächen: harte Wände auch für
                // Flächen, denen die Kante nicht selbst gehört.
                let global_chain_edges: HashSet<(u32, u32)> = face_loops
                    .iter()
                    .flat_map(|rings| rings.iter())
                    .flat_map(|(r, _)| {
                        (0..r.len()).map(move |k| {
                            let (a, b) = (r[k], r[(k + 1) % r.len()]);
                            (a.min(b), a.max(b))
                        })
                    })
                    .filter(|&(a, b)| a != b)
                    .collect();
                let (mut holes, mut nonmani) = (0usize, 0usize);
                let mut open_edges: Vec<(u32, u32)> = Vec::new();
                let mut weak_tris: Vec<[u32; 3]> = Vec::new();
                for gf in 0..face_surfs.len() {
                    let loops: Vec<Vec<u32>> =
                        face_loops[gf].iter().map(|(r, _)| r.clone()).collect();
                    // Mitglieder + Pro-Flächen-Normalen (Randpunkte tragen die
                    // Normale DIESER Fläche aus dem Trailing-Record; Innen-
                    // punkte ihr ∇f).
                    let mut members: Vec<u32> = Vec::new();
                    let mut nmap: HashMap<u32, [f32; 3]> = HashMap::new();
                    for ring in &loops {
                        for &gi in ring {
                            if nmap.contains_key(&gi) {
                                continue;
                            }
                            let n = point_faces[gi as usize]
                                .iter()
                                .find(|&&(g, _)| g == gf as u32)
                                .map(|&(_, n)| n)
                                .unwrap_or(all[gi as usize].n);
                            nmap.insert(gi, n);
                            members.push(gi);
                        }
                    }
                    // Interior-Normalen aus der TRÄGERFLÄCHE (exakt), nicht
                    // aus dem gespeicherten Punkt-∇f: Default-/Rausch-Normalen
                    // ([0,0,1]-LPoint-Default!) machten den Cocone-Filter auf
                    // 4 von 6 Box-Flächen blind — leere Sterne, 0 Dreiecke.
                    let fsn = |p_world: [f32; 3]| -> [f32; 3] {
                        let fs = &face_surfs[gf];
                        let pl = crate::world_to_local(&fs.placement, p_world);
                        let ps = crate::world_to_local(&fs.transform, pl);
                        let e = 1e-3f32;
                        let sd = |q: [f32; 3]| crate::csg::sdf_halfspace(&fs.surface, q);
                        let g = [
                            sd([ps[0] + e, ps[1], ps[2]]) - sd([ps[0] - e, ps[1], ps[2]]),
                            sd([ps[0], ps[1] + e, ps[2]]) - sd([ps[0], ps[1] - e, ps[2]]),
                            sd([ps[0], ps[1], ps[2] + e]) - sd([ps[0], ps[1], ps[2] - e]),
                        ];
                        let gl = crate::quat_rotate(fs.transform.rotation, g);
                        lichen::unit(crate::quat_rotate(fs.placement.rotation, gl))
                    };
                    for &gi in &face_interior[gf] {
                        if nmap.insert(gi, fsn(all[gi as usize].p)).is_none() {
                            members.push(gi);
                        }
                    }
                    if members.len() < 3 {
                        continue;
                    }
                    let own = |p: [f32; 3]| face_of(p) == Some(gf as u32);
                    // "auto" (Default-Empfehlung): Akzeptanz nach Trägertyp —
                    // DOPPELT gekrümmte Flächen (Kugel/Torus/Gyroid) → Umkugel
                    // mit Fächer-Tie-Break (gemessen wasserdicht auf freien
                    // Kugeln r1/r8/r50: 0/0, χ=2 — die Tangential-Sterne
                    // divergieren dort, Ties sind nie exakt); planare und
                    // ABWICKELBARE Flächen (Plane/Zylinder/Kegel) → Stern-
                    // Konsens (dort sind Hex-Ties exakt cozirkulär, der
                    // Fächer-Partner oft kein Stern-Kandidat: Box 1244 vs.
                    // 128 offene Kanten).
                    let face_accept: &str = if opts.accept == "auto" {
                        match &face_surfs[gf].surface {
                            HalfSpace::Plane { .. }
                            | HalfSpace::InfiniteCylinder { .. }
                            | HalfSpace::Cone { .. } => "vote",
                            _ => "gabriel",
                        }
                    } else {
                        &opts.accept
                    };
                    #[cfg(not(target_arch = "wasm32"))]
                    let t_t = std::time::Instant::now();
                    let res = crate::lichen_tri::triangulate_face_general(
                        &positions,
                        &nmap,
                        &spacings,
                        &members,
                        &loops,
                        opts.curv_angle_deg.to_radians(),
                        face_accept,
                        Some(&own),
                        Some(&global_chain_edges),
                        opts.fill_holes,
                        opts.extract_walk,
                        &fg,
                        &|p: [f32; 3]| {
                            let fs = &face_surfs[gf];
                            let pl = crate::world_to_local(&fs.placement, p);
                            let ps = crate::world_to_local(&fs.transform, pl);
                            crate::csg::sdf_halfspace(&fs.surface, ps).abs() <= 0.05 * h
                        },
                    );
                    if std::env::var("LICHEN_DEBUG_FACES").is_ok() {
                        eprintln!(
                            "FACE gf={gf} {:?} members={} interior={} tris={} holes={}",
                            std::mem::discriminant(&face_surfs[gf].surface),
                            members.len(), face_interior[gf].len(),
                            res.tris.len(), res.hole_edges
                        );
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    if std::env::var("LICHEN_PERF").is_ok() {
                        eprintln!(
                            "PERF tri gf={gf}: {:?} ({} members, {} tris)",
                            t_t.elapsed(), members.len(), res.tris.len()
                        );
                    }
                    holes += res.hole_edges;
                    nonmani += res.nonmanifold_edges;
                    open_edges.extend(res.open_edges);
                    weak_tris.extend(res.weak_tris);
                    t.extend(res.tris);
                }
                // Flächen-übergreifende DUPLIKATE konsolidieren (kein
                // Qualitäts-Filter — dasselbe Dreieck aus zwei Sichten IST
                // ein Dreieck): ein Dreieck aus 3 Ketten-Punkten liegt in
                // den Member-Mengen BEIDER Nachbarflächen und wird von
                // beiden Sternläufen akzeptiert; kanonischer Schlüssel =
                // sortiertes Index-Tripel (Demo-Projekt: 305 Duplikate an
                // den Mikro-Feature-Ketten → 201 non-manifolde Kanten).
                let mut seen: std::collections::HashSet<[u32; 3]> =
                    std::collections::HashSet::new();
                t.retain(|&[a, b, c]| {
                    let mut k = [a, b, c];
                    k.sort_unstable();
                    seen.insert(k)
                });
                // Orientierung global konsistent machen (auch über Nähte).
                lichen::orient_consistent(&all, &mut t);
                if holes + nonmani > 0 {
                    eprintln!(
                        "lichen general-tri: {holes} Loch-Kanten, {nonmani} non-manifold Kanten (Diagnose)"
                    );
                }
                return lichen::pack_with_diag(&all, &t, &point_faces, &open_edges, &weak_tris);
            }
            // Diagnose-Sammlung der Fallback-Flächen (Overlay: orange).
            let mut fb_weak: Vec<[u32; 3]> = Vec::new();
            // Globale Ketten-Kanten (für den chartlosen Pro-Flächen-Fallback
            // unten — fremde Ketten sind dort harte Wände, wie im General-Pfad).
            let global_chain_edges: HashSet<(u32, u32)> = face_loops
                .iter()
                .flat_map(|rings| rings.iter())
                .flat_map(|(r, _)| {
                    (0..r.len()).map(move |k| {
                        let (a, b) = (r[k], r[(k + 1) % r.len()]);
                        (a.min(b), a.max(b))
                    })
                })
                .filter(|&(a, b)| a != b)
                .collect();
            for gf in 0..face_surfs.len() {
                // Rand-Clearance des CDT PRO FLÄCHE aus dem lokalen Punkt-
                // Spacing, nicht pauschal 0.25·h (Ticket e6b21b96): das
                // Wachstum verdichtet krümmungsadaptiv weit unter h (r-Floor =
                // Min. Kantenabstand) — eine 0.25·h-Clearance verwarf dann
                // JEDEN gewachsenen Punkt der schmalen Flächen als „zu nah am
                // Rand" und der CDT überspannte die Fläche nur mit Rand-
                // Ketten-Dreiecken.
                let clearance = 0.25
                    * face_interior[gf]
                        .iter()
                        .fold(h, |m, &gi| m.min(all[gi as usize].space));
                let loops: Vec<Vec<u32>> =
                    face_loops[gf].iter().map(|(r, _)| r.clone()).collect();
                // Pro-Fläche-Normalen der Ketten-Punkte (für die Innenseiten-
                // Entscheidung der Ketten-Kanten-Einzelnutzung).
                let chain_normals = {
                    let mut m: HashMap<u32, [f32; 3]> = HashMap::new();
                    for ring in &loops {
                        for &gi in ring {
                            let n = point_faces[gi as usize]
                                .iter()
                                .find(|&&(g, _)| g == gf as u32)
                                .map(|&(_, n)| n)
                                .unwrap_or(all[gi as usize].n);
                            m.insert(gi, n);
                        }
                    }
                    m
                };
                if matches!(face_surfs[gf].surface, HalfSpace::Plane { .. }) {
                    let positions: Vec<[f32; 3]> = all.iter().map(|lp| lp.p).collect();
                    let mut ft = crate::mesher_cdt::triangulate_planar_face(
                        &positions,
                        &loops,
                        &face_interior[gf],
                        clearance as f64,
                    );
                    // Ketten-Kanten-Einzelnutzung: Rand-Folds (Sliver auf der
                    // Außenseite, Parität am Rand = Münzwurf) entfernen —
                    // dieselbe Regel wie im General-Pfad.
                    crate::lichen_tri::enforce_chain_single_use(
                        &positions,
                        &loops,
                        &chain_normals,
                        &mut ft,
                    );
                    t.extend(ft);
                    continue;
                }
                // Chart-Versuch als benannter Block: schlägt er fehl
                // (Vollwickel-Kugel, Torus, Gyroid, loop-lose Fläche), fällt
                // die Fläche unten auf die CHARTLOSE General-Triangulierung
                // zurück, statt ungemesht zu bleiben (Betreiber-Bilder:
                // Meridian-Spalt in der Mini-Kugel, Gyroid ganz ohne Mesh).
                let charted = 'chart: {
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
                    if std::env::var("LICHEN_DEBUG_FACES").is_ok() {
                        eprintln!("CHART gf={gf}: kein Chart (loops={})", loops.len());
                    }
                    break 'chart false; // Torus/Gyroid/loop-los → Fallback
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
                // Liveness-Prädikat (Ticket 28a95b4e): ein Chart-Zentroid ist
                // Teil der Fläche, wenn sein Träger-Punkt auf dem BODY liegt.
                // `world_of` liefert den Punkt exakt AUF dem Träger (Chart→
                // Welt), also genügt der Body-Feldtest — Sehnen-Fehler der
                // Dreiecke spielen keine Rolle. Auf geschlossenen Trägern
                // (Kugel) ist die outer/inner-Parität mehrdeutig: sie füllte
                // den VERGRABENEN Annulus statt Kappe + Bohrungs-Stück.
                // Enge Toleranz: live Zentroide liegen via world_of EXAKT auf
                // dem Träger (Body-|f| ≈ f32-Rauschen), vergrabene ab ~0.05 tief.
                let live_tol = 0.02f32;
                let live = |p: [f32; 3]| -> bool { fg(p).0.abs() <= live_tol };
                let Some(mesh) = crate::mesher_cdt::triangulate_periodic_face(
                    &scaled,
                    &scaled_interior,
                    period,
                    h_chart,
                    (clearance / ms) as f64,
                    &poles,
                    all.len() as u32,
                    &world_of,
                    Some(&live),
                ) else {
                    break 'chart false; // Vollwickel o.ä. → Fallback
                };
                // Naht-Steiner-/Pol-Punkte als neue Flächenpunkte anhängen
                // (flag 1); Normale aus ∇f.
                for p in mesh.aux {
                    let (d, g) = fg(p);
                    let n = lichen::unit(g);
                    all.push(LPoint { p, sdf: d, n, u: h, space: h, flag: 1.0 });
                    point_faces.push(vec![(gf as u32, n)]);
                    local_pos.push([0.0; 3]);
                }
                // Ketten-Kanten-Einzelnutzung wie im planaren Zweig
                // (Positionen NACH dem Aux-Anhängen, die Indizes der
                // Naht-/Pol-Punkte zeigen in `all`).
                let positions: Vec<[f32; 3]> = all.iter().map(|lp| lp.p).collect();
                let mut ft = mesh.tris;
                crate::lichen_tri::enforce_chain_single_use(
                    &positions,
                    &loops,
                    &chain_normals,
                    &mut ft,
                );
                t.extend(ft);
                true
                };
                if charted {
                    continue;
                }
                // ── CHARTLOS-FALLBACK pro Fläche: General-Triangulierung
                // (Stern/Umkugel, auto-Akzeptanz) — identische Mechanik wie
                // der General-Pfad, nur für die Flächen, die kein Chart
                // tragen. So liefert der Default-Modus (General tri aus)
                // auch auf Vollwickel-Kugeln, Torus und Gyroid ein Mesh
                // statt Spalt/Leere. ──
                let positions: Vec<[f32; 3]> = all.iter().map(|lp| lp.p).collect();
                let spacings: Vec<f32> = all.iter().map(|lp| lp.space).collect();
                let mut members: Vec<u32> = Vec::new();
                let mut nmap: HashMap<u32, [f32; 3]> = HashMap::new();
                for ring in &loops {
                    for &gi in ring {
                        if nmap.contains_key(&gi) {
                            continue;
                        }
                        let n = point_faces[gi as usize]
                            .iter()
                            .find(|&&(g, _)| g == gf as u32)
                            .map(|&(_, n)| n)
                            .unwrap_or(all[gi as usize].n);
                        nmap.insert(gi, n);
                        members.push(gi);
                    }
                }
                let fsn = |p_world: [f32; 3]| -> [f32; 3] {
                    let fs = &face_surfs[gf];
                    let pl = crate::world_to_local(&fs.placement, p_world);
                    let ps = crate::world_to_local(&fs.transform, pl);
                    let e = 1e-3f32;
                    let sd = |q: [f32; 3]| crate::csg::sdf_halfspace(&fs.surface, q);
                    let g = [
                        sd([ps[0] + e, ps[1], ps[2]]) - sd([ps[0] - e, ps[1], ps[2]]),
                        sd([ps[0], ps[1] + e, ps[2]]) - sd([ps[0], ps[1] - e, ps[2]]),
                        sd([ps[0], ps[1], ps[2] + e]) - sd([ps[0], ps[1], ps[2] - e]),
                    ];
                    let gl = crate::quat_rotate(fs.transform.rotation, g);
                    lichen::unit(crate::quat_rotate(fs.placement.rotation, gl))
                };
                for &gi in &face_interior[gf] {
                    if nmap.insert(gi, fsn(all[gi as usize].p)).is_none() {
                        members.push(gi);
                    }
                }
                if members.len() < 3 {
                    continue;
                }
                let own = |p: [f32; 3]| face_of(p) == Some(gf as u32);
                let face_accept: &str = match &face_surfs[gf].surface {
                    HalfSpace::Plane { .. }
                    | HalfSpace::InfiniteCylinder { .. }
                    | HalfSpace::Cone { .. } => "vote",
                    _ => "gabriel",
                };
                let res = crate::lichen_tri::triangulate_face_general(
                    &positions,
                    &nmap,
                    &spacings,
                    &members,
                    &loops,
                    opts.curv_angle_deg.to_radians(),
                    face_accept,
                    Some(&own),
                    Some(&global_chain_edges),
                    opts.fill_holes,
                    opts.extract_walk,
                    &fg,
                    &|p: [f32; 3]| {
                        let fs = &face_surfs[gf];
                        let pl = crate::world_to_local(&fs.placement, p);
                        let ps = crate::world_to_local(&fs.transform, pl);
                        crate::csg::sdf_halfspace(&fs.surface, ps).abs() <= 0.05 * h
                    },
                );
                if std::env::var("LICHEN_DEBUG_FACES").is_ok() {
                    eprintln!(
                        "FALLBACK gf={gf} {:?} members={} tris={} holes={}",
                        std::mem::discriminant(&face_surfs[gf].surface),
                        members.len(),
                        res.tris.len(),
                        res.hole_edges
                    );
                }
                fb_weak.extend(res.weak_tris);
                t.extend(res.tris);
            }
            diag_weak = fb_weak;
            t
        } else {
            Vec::new()
        };

        // Flächen-übergreifende exakte DUPLIKATE konsolidieren (auch der
        // Chart-Pfad erzeugt sie: ein Dreieck aus 3 Ketten-Punkten einer
        // Mikro-Fläche entsteht identisch im CDT BEIDER Nachbarflächen —
        // Demo-Projekt: 24–27 non-manifolde Kanten allein daraus).
        let tris = {
            let mut seen: std::collections::HashSet<[u32; 3]> =
                std::collections::HashSet::new();
            let mut t = tris;
            t.retain(|&[a, b, c]| {
                let mut k = [a, b, c];
                k.sort_unstable();
                seen.insert(k)
            });
            // Orientierung global konsistent machen (auch über Nähte).
            lichen::orient_consistent(&all, &mut t);
            t
        };
        // Watertight-Verstöße farbig (Betreiber-Wunsch): finale offene Kanten
        // ROT auch im Chart-Modus — bisher zeichnete nur der General-Pfad
        // seine Diagnose.
        let open: Vec<(u32, u32)> = {
            let mut edge_use: HashMap<(u32, u32), u32> = HashMap::new();
            for &[a, b, c] in &tris {
                for (x, y) in [(a, b), (b, c), (c, a)] {
                    *edge_use.entry((x.min(y), x.max(y))).or_default() += 1;
                }
            }
            let mut open: Vec<(u32, u32)> =
                edge_use.iter().filter(|(_, &u)| u == 1).map(|(&e, _)| e).collect();
            open.sort_unstable();
            open
        };
        lichen::pack_with_diag(&all, &tris, &point_faces, &open, &diag_weak)
    }
}

#[cfg(test)]
mod tests {
    use fielddraft_protocol::{BoolOp, Primitive};

    /// Demo-Projekt mit den DIALOG-Settings des Betreibers (2026-07-11:
    /// Krümmungswinkel 5°, min. Kantenabstand 0,2, Reach an) — dichter
    /// abgetastet als die 20°-Gates. Historie: 25 Orientierungs-Folds an
    /// der tangentialen Mini-Kugel-Naht („Dreiecke durch die Kugel") —
    /// seit v0.6.538 GRÜN: die Orientierungs-Propagation LÖST Konflikte
    /// auf (pro Fold fliegt das Dreieck mit größerem Umkreis; ehrliches
    /// Loch statt falscher Geometrie) + Regionen-Zugehörigkeit der
    /// Sichtlinie (projizierter Sehnen-Mittelpunkt muss auf der eigenen
    /// Trägerfläche liegen). Läuft parallel zur Suite (~4 min, teuerster
    /// Einzeltest — der Preis der dichten Abtastung).
    #[test]
    fn lichen_demo_user_settings_orientation_clean() {
        let mut k = demo_project_kernel();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":10.0,"grow":true,"curvAngleDeg":5.0,"minEdgeSpacing":0.2,"useReach":true,"reachTol":1.0,"maxPoints":50000,"generalTri":true,"fillHoles":false}"#.into(),
        );
        let (tris, q) = quality_of(&out);
        eprintln!("user-settings general: tris={tris} {q:?}");
        assert_eq!(q.orientation_violations, 0, "{q:?}");
        assert_eq!(q.nonmanifold_edges, 0, "{q:?}");
    }

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
        // Layout: header(3) + points×9 + indices×3 + nFaceRec + faceRec×4
        //         + nOpenEdge + openEdge×2 + nWeakTri + weakTri×3
        //         + nLongEdge + longEdge×2 (Diagnose) + Qualitäts-Block (6).
        let idx_end = 3 + total * 9 + tris * 3;
        let n_face = out[idx_end] as usize;
        let diag = idx_end + 1 + n_face * 4;
        let n_open = out[diag] as usize;
        let weak_base = diag + 1 + n_open * 2;
        let n_weak = out[weak_base] as usize;
        let long_base = weak_base + 1 + n_weak * 3;
        let n_long = out[long_base] as usize;
        assert_eq!(
            long_base + 1 + n_long * 2 + 6,
            out.len(),
            "flat layout: points + index + face-normal block + diag + quality block"
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
        // ECHTES CSG via load_json mit targets: add_primitive(TAG_REMOVE)
        // hängt ohne targets KEIN Boolean an — der Test lief historisch mit
        // zwei getrennten Bodies (freie Kugel + Box) und war ein False
        // Positive: nur die Box war geschlossen, die Kugel blieb komplett
        // ungemesht (Kappen-Loops existierten nie). Aufgeflogen 2026-07-12
        // beim Chartlos-Fallback (der die freie Kugel plötzlich meshte).
        let mut k = crate::Kernel::new();
        k.load_json(
            r#"{
            "schema_version": 1,
            "name": "hemi",
            "feature_calls": [
                { "type": "sphere", "id": 1, "params": {
                    "op": "new", "radius": 10, "translate": [0, 0, 0] } },
                { "type": "box", "id": 2, "params": {
                    "op": "remove", "targets": "qCreatedBy(1)",
                    "half_extents": [25, 20, 25], "translate": [0, -20, 0] } }
            ],
            "next_feature_id": 3
        }"#,
        )
        .unwrap();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":2.5,"grow":true,"maxPoints":20000}"#.into(),
        );
        assert_watertight(&out);
    }

    /// Ticket ee07b6f1: Wachstum auf einem Teil, das KLEINER als h ist — der
    /// frühere 0.2·h-Floor klemmte den Wachstumsradius auf 2 mm (bei h=10)
    /// und verwarf die Krümmungs-Verfeinerung: auf einem kleinen gekrümmten
    /// Körper wuchs genau EIN Innenpunkt. Jetzt floort der Min. Kantenabstand,
    /// R_curv greift, und ein r=1.5-Zylinder bekommt bei h=10 eine echte
    /// Punktwolke.
    #[test]
    fn growth_refines_below_h_on_small_curved_part() {
        let mut k = crate::Kernel::new();
        k.add_primitive(
            Primitive::KIND_CYLINDER,
            BoolOp::TAG_NEW,
            0.0,
            0.0,
            0.0,
            1.5,
            3.0,
            0.0,
        )
        .unwrap();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":10.0,"grow":true,"curvAngleDeg":20.0,"minEdgeSpacing":0.05,"maxPoints":20000}"#.into(),
        );
        let total = out[0] as usize;
        let surf = (0..total).filter(|&i| out[3 + i * 9 + 8] as i32 == 1).count();
        assert!(
            surf > 30,
            "Krümmungs-Verfeinerung muss auf dem kleinen Zylinder greifen, got {surf} Innenpunkte"
        );
        // Ticket e6b21b96: die gewachsenen Punkte müssen auch im CDT LANDEN —
        // die frühere 0.25·h-Clearance verwarf bei dichtem Wachstum (Spacing
        // ≪ h) jeden Innenpunkt als „zu nah am Rand", und die Dreiecke
        // überspannten die Fläche nur mit Rand-Ketten.
        let tris = out[1] as usize;
        assert!(tris > 0, "Triangulierung erwartet");
        let base = 3 + total * 9;
        let used_surface = (0..tris)
            .flat_map(|t| (0..3).map(move |k| t * 3 + k))
            .filter(|&ix| {
                let pi = out[base + ix] as usize;
                pi < total && out[3 + pi * 9 + 8] as i32 == 1
            })
            .count();
        assert!(
            used_surface > 30,
            "Dreiecke müssen gewachsene Punkte nutzen, got {used_surface} Referenzen"
        );
    }

    /// ALLGEMEINER Pfad (Stern-Konsistenz, chartlos): die Box — der leicht
    /// prüfbare Planarfall und Referenz gegen den CDT — muss auch dort ein
    /// GESCHLOSSENES 2-Manifold liefern (Euler 2, keine Loch-Kante). Auf
    /// planaren Flächen ist die Tangentialebene konstant, alle Sterne stimmen
    /// überein → das Ergebnis ist ein echtes Delaunay der Fläche.
    #[test]
    #[ignore = "WIP: nach Dichte-Normalisierung (Trägerflächen-Krümmung, v0.6.530) \
                108 offene Kanten bei sauberer Struktur (0/0/0); Front-Kollisions- \
                Lücken im Flächen-Inneren — Rebalancing-Feinschliff steht aus."]
    fn lichen_general_tri_box_is_watertight() {
        let mut k = box_kernel();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":3.0,"grow":true,"maxPoints":8000,"generalTri":true}"#.into(),
        );
        assert_watertight(&out);
    }

    /// General-Pfad auf gekrümmten Flächen: Zylinder (Band + Kappen) — gleiche
    /// Watertight-Messlatte wie der Chart-Pfad, mit demselben chartlosen Code.
    /// SEIT v0.6.530 GRÜN: Trägerflächen-Krümmung + Dichte-Normalisierung
    /// machten den Zylinder (Mantel + beide Kappen) im General-Pfad
    /// WASSERDICHT (χ=2) — vorher 345–1000 offene Kanten.
    #[test]
    fn lichen_general_tri_cylinder_is_watertight() {
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
            r#"{"targetEdgeLength":3.0,"grow":true,"maxPoints":20000,"generalTri":true}"#.into(),
        );
        assert_watertight(&out);
    }

    /// General-Pfad-Smoke: läuft, produziert Dreiecke, KEINE non-manifolden
    /// Kanten (die Stern-Konsistenz-Garantie — Löcher sind das offene WIP,
    /// Überlappungen wären ein Regressions-Alarm).
    #[test]
    fn lichen_general_tri_box_smoke_no_nonmanifold() {
        let mut k = box_kernel();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":3.0,"grow":true,"maxPoints":8000,"generalTri":true}"#.into(),
        );
        let total = out[0] as usize;
        let tris = out[1] as usize;
        assert!(tris > 100, "General-Pfad muss substanziell triangulieren, got {tris}");
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
        let over = edge_use.values().filter(|&&u| u > 2).count();
        assert_eq!(over, 0, "Stern-Konsistenz darf keine non-manifolden Kanten erzeugen");
    }

    /// Betreiber-Demo-Projekt (2026-07-11): Kugel r50 ∩ Gyroid (cell 10),
    /// Zylinder-Bohrung r30, Mikro-Box-Intersect (6×8×6 an der Oberfläche),
    /// Mini-Kugel (r50 skaliert 0.02) — GENAU das Projekt, mit dem der
    /// Betreiber das Lichen-Meshing in der App testet. Der Feature-Teil ist
    /// bewusst HART im Test verdrahtet (Betreiber-Wunsch): der Testfall ist
    /// hier lesbar und kann nicht stillschweigend von einer externen Datei
    /// wegdriften. Regressions-Gate für die Mesh-QUALITÄT
    /// (lichen::mesh_quality) beider Triangulierungs-Pfade.
    fn demo_project_kernel() -> crate::Kernel {
        let json = r#"{
            "schema_version": 1,
            "name": "Lichen-Demo",
            "feature_calls": [
                { "type": "sphere", "id": 1, "params": {
                    "op": "new", "radius": 50, "translate": [0, 0, 0] } },
                { "type": "gyroid", "id": 2, "params": {
                    "op": "intersect", "targets": "qCreatedBy(1)",
                    "cell_size": { "unit": "mm", "value": 10 },
                    "thickness": { "unit": "mm", "value": 1 },
                    "blend_radius": { "unit": "mm", "value": 0 },
                    "chamfer_size": { "unit": "mm", "value": 0 },
                    "displace_depth": { "unit": "mm", "value": 0 },
                    "shell_thickness": { "unit": "mm", "value": 0 } } },
                { "type": "cylinder", "id": 3, "params": {
                    "op": "remove", "targets": "qCreatedBy(1)",
                    "radius": 30, "half_height": 60, "translate": [0, 0, 0] } },
                { "type": "box", "id": 4, "params": {
                    "op": "intersect", "targets": "qCreatedBy(1)",
                    "half_extents": [3, 4, 3], "translate": [0, 0, 0] } },
                { "type": "translate", "id": 5, "params": {
                    "target_id": 4,
                    "translate": [7.970110677489131, 22.314295, 29.869688] } },
                { "type": "sphere", "id": 6, "params": {
                    "op": "add", "targets": "qCreatedBy(2)",
                    "radius": 50, "translate": [0, 0, 0] } },
                { "type": "translate", "id": 7, "params": {
                    "target_id": 6,
                    "translate": [5.2107882, 22.11431, 29.24847473977133] } },
                { "type": "scale", "id": 8, "params": {
                    "target_id": 6, "scale": [0.02, 0.02, 0.02] } }
            ],
            "next_feature_id": 10
        }"#;
        let mut k = crate::Kernel::new();
        k.load_json(json).expect("Demo-Projekt lädt");
        k
    }

    /// Flat-Buffer → (Positionen, Dreiecke, Qualität).
    fn quality_of(out: &[f32]) -> (usize, crate::lichen::MeshQuality) {
        let total = out[0] as usize;
        let tris = out[1] as usize;
        let positions: Vec<[f32; 3]> = (0..total)
            .map(|i| [out[3 + i * 9], out[3 + i * 9 + 1], out[3 + i * 9 + 2]])
            .collect();
        let base = 3 + total * 9;
        let t: Vec<[u32; 3]> = (0..tris)
            .map(|k| {
                [
                    out[base + k * 3] as u32,
                    out[base + k * 3 + 1] as u32,
                    out[base + k * 3 + 2] as u32,
                ]
            })
            .collect();
        (tris, crate::lichen::mesh_quality(&positions, &t))
    }

    /// Chart-Pfad (Default) auf dem Demo-Projekt — Qualitäts-Gate. Stand
    /// 2026-07-11 (h=10) nach Ketten-Kanten-Einzelnutzung + Orientierungs-
    /// Propagation: tris=1057, boundary=131 (Gyroid-Nähte, F2 offen),
    /// nonmanifold=0, degenerate=0, orientierung=0.
    #[test]
    fn lichen_demo_project_chart_quality() {
        let mut k = demo_project_kernel();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":10.0,"grow":true,"maxPoints":50000}"#.into(),
        );
        let (tris, q) = quality_of(&out);
        eprintln!("demo chart: tris={tris} {q:?}");
        // tris-Schwelle an die NORMALISIERTE Dichte angepasst (v0.6.530: die
        // frühere 15×-Überdichte ist weg; gemessen 415).
        assert!(tris > 300, "Chart-Pfad muss substanziell triangulieren, got {tris}");
        assert_eq!(q.degenerate_tris, 0, "degenerierte Dreiecke: {q:?}");
        assert_eq!(q.nonmanifold_edges, 0, "non-manifold (Ketten-Einzelnutzung!): {q:?}");
        assert_eq!(q.orientation_violations, 0, "Orientierung (Propagation!): {q:?}");
        assert!(q.boundary_edges <= 250, "Rand-Kanten REGRESSION (Stand 131): {q:?}");
    }

    /// General-Pfad (Stern-Konsistenz) auf dem Demo-Projekt — Qualitäts-
    /// Gate. Stand 2026-07-11 (h=10) nach Owner-Stitching (Ketten-Kanten-
    /// Einzelnutzung je Fläche + fremde Ketten als globale Wände + Filter
    /// nach dem Loch-Füller) und Orientierungs-Propagation: tris=917,
    /// boundary=165 (Gyroid-Nähte + Stern-Löcher), nonmanifold=0,
    /// degenerate=0, orientierung=0.
    #[test]
    fn lichen_demo_project_general_quality() {
        let mut k = demo_project_kernel();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":10.0,"grow":true,"maxPoints":50000,"generalTri":true}"#.into(),
        );
        let (tris, q) = quality_of(&out);
        eprintln!("demo general: tris={tris} {q:?}");
        assert!(tris > 400, "General-Pfad muss substanziell triangulieren, got {tris}");
        assert_eq!(q.degenerate_tris, 0, "degenerierte Dreiecke: {q:?}");
        assert_eq!(q.nonmanifold_edges, 0, "non-manifold (Ketten-Einzelnutzung!): {q:?}");
        assert_eq!(q.orientation_violations, 0, "Orientierung (Propagation!): {q:?}");
        assert!(q.boundary_edges <= 300, "Rand-Kanten REGRESSION (Stand 165): {q:?}");
    }

    /// Betreiber-Bild 2026-07-12: der Grow-Algorithmus emittierte Punkte
    /// NEBEN dem Körper (Luft-Punkte an konkaven Buchten). Wurzel: die
    /// Projektions-Toleranz `|f| ≤ 0.2·rr` wächst mit dem Wachstumsradius —
    /// bei h=10 akzeptierte sie 2 mm Abstand. Hartes Gate: JEDER gewachsene
    /// Punkt liegt auf der Oberfläche.
    #[test]
    fn lichen_grown_points_lie_on_surface() {
        let mut k = demo_project_kernel();
        for (name, opts) in [
            (
                "chart",
                r#"{"targetEdgeLength":10.0,"grow":true,"curvAngleDeg":5.0,"minEdgeSpacing":0.2,"maxPoints":50000}"#,
            ),
            (
                "general",
                r#"{"targetEdgeLength":10.0,"grow":true,"curvAngleDeg":5.0,"minEdgeSpacing":0.2,"maxPoints":50000,"generalTri":true,"fillHoles":false}"#,
            ),
        ] {
            let out = k.lichen_mesh(opts.into());
            let total = out[0] as usize;
            let mut worst = [0.0f32; 4]; // Index = flag (1 surf, 2 chain, 3 corner)
            let mut worst_p = [0.0f32; 3];
            for i in 0..total {
                let sdf = out[3 + i * 9 + 3].abs();
                let flag = (out[3 + i * 9 + 8] as usize).min(3);
                if flag == 1 && sdf > worst[1] {
                    worst_p = [out[3 + i * 9], out[3 + i * 9 + 1], out[3 + i * 9 + 2]];
                }
                worst[flag] = worst[flag].max(sdf);
            }
            eprintln!(
                "{name}: max |sdf| über {total} Punkte: surf={} chain={} corner={} worst_p={worst_p:?}",
                worst[1], worst[2], worst[3]
            );
            assert!(
                worst[1] <= 0.1,
                "{name}: Punkte müssen auf der Oberfläche liegen (|f| ≤ 0.01·h), max {} bei {worst_p:?}",
                worst[1]
            );
        }
    }

    /// SCRATCH-Analyse (Betreiber-Bild 3: Spalt in der Mini-Kugel mit
    /// degenerierten langen Dreiecken, Chart-Pfad): Dreiecke an der
    /// Mini-Kugel vermessen — lange Kanten relativ zum lokalen Spacing.
    /// Demo OHNE Mikro-Box-Intersect = Betreiber-Screenshot-Szene
    /// (volle Gyroid-Wand + Mini-Kugel r=1).
    fn demo_no_micro_kernel() -> crate::Kernel {
        let json = r#"{
            "schema_version": 1,
            "name": "Lichen-Demo-ohne-Mikro",
            "feature_calls": [
                { "type": "sphere", "id": 1, "params": {
                    "op": "new", "radius": 50, "translate": [0, 0, 0] } },
                { "type": "gyroid", "id": 2, "params": {
                    "op": "intersect", "targets": "qCreatedBy(1)",
                    "cell_size": { "unit": "mm", "value": 10 },
                    "thickness": { "unit": "mm", "value": 1 },
                    "blend_radius": { "unit": "mm", "value": 0 },
                    "chamfer_size": { "unit": "mm", "value": 0 },
                    "displace_depth": { "unit": "mm", "value": 0 },
                    "shell_thickness": { "unit": "mm", "value": 0 } } },
                { "type": "cylinder", "id": 3, "params": {
                    "op": "remove", "targets": "qCreatedBy(1)",
                    "radius": 30, "half_height": 60, "translate": [0, 0, 0] } },
                { "type": "sphere", "id": 6, "params": {
                    "op": "add", "targets": "qCreatedBy(2)",
                    "radius": 50, "translate": [0, 0, 0] } },
                { "type": "translate", "id": 7, "params": {
                    "target_id": 6,
                    "translate": [5.2107882, 22.11431, 29.24847473977133] } },
                { "type": "scale", "id": 8, "params": {
                    "target_id": 6, "scale": [0.02, 0.02, 0.02] } }
            ],
            "next_feature_id": 10
        }"#;
        let mut k = crate::Kernel::new();
        k.load_json(json).expect("Demo-ohne-Mikro lädt");
        k
    }

    #[test]
    #[ignore = "Analyse-Werkzeug, kein Gate"]
    fn lichen_scratch_minisphere_chart_analysis() {
        let mut k = if std::env::var("LICHEN_SCRATCH_NO_MICRO").is_ok() {
            demo_no_micro_kernel()
        } else {
            demo_project_kernel()
        };
        let curv: f32 = std::env::var("LICHEN_SCRATCH_CURV")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5.0);
        let minsp: f32 = std::env::var("LICHEN_SCRATCH_MINSP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.1);
        let gen = std::env::var("LICHEN_SCRATCH_GENERAL").is_ok();
        let out = k.lichen_mesh(format!(
            r#"{{"targetEdgeLength":10.0,"grow":true,"curvAngleDeg":{curv},"minEdgeSpacing":{minsp},"maxPoints":50000,"generalTri":{gen},"fillHoles":false}}"#
        ));
        let total = out[0] as usize;
        let tris = out[1] as usize;
        let pos = |i: usize| -> [f32; 3] { [out[3 + i * 9], out[3 + i * 9 + 1], out[3 + i * 9 + 2]] };
        let c = [5.2107882f32, 22.11431, 29.24847];
        // Punkt-Statistik an der Mini-Kugel: Anzahl + space-Histogramm.
        {
            let mut n_near = [0usize; 4];
            let mut space_hist = std::collections::BTreeMap::new();
            for i in 0..total {
                let p = pos(i);
                let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
                let r = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if (r - 1.0).abs() > 0.05 {
                    continue;
                }
                let flag = (out[3 + i * 9 + 8] as usize).min(3);
                n_near[flag] += 1;
                if flag == 1 {
                    let sp = out[3 + i * 9 + 7];
                    *space_hist.entry(format!("{:.2}", sp)).or_insert(0usize) += 1;
                }
            }
            eprintln!(
                "mini r=1±0.05: surf={} chain={} corner={} u-hist(surf)={space_hist:?}",
                n_near[1], n_near[2], n_near[3]
            );
            // CSV-Dump für Kappen-Dichte-Analyse.
            use std::io::Write;
            let mut f = std::fs::File::create("/tmp/mini_pts.csv").unwrap();
            for i in 0..total {
                let p = pos(i);
                let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
                let r = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                if (r - 1.0).abs() > 0.05 {
                    continue;
                }
                writeln!(f, "{},{},{},{},{}", i, d[0], d[1], d[2], out[3 + i * 9 + 8]).unwrap();
            }
            // Dreiecke der Mini-Kugel (alle 3 Ecken nah) für Kanten-Längen je Region.
            let mut f2 = std::fs::File::create("/tmp/mini_tris.csv").unwrap();
            let base2 = 3 + total * 9;
            for t in 0..tris {
                let idx: Vec<usize> = (0..3).map(|j| out[base2 + t * 3 + j] as usize).collect();
                let ok = idx.iter().all(|&i| {
                    let p = pos(i);
                    let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
                    ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() - 1.0).abs() < 0.05
                });
                if ok {
                    writeln!(f2, "{},{},{}", idx[0], idx[1], idx[2]).unwrap();
                }
            }
        }
        let near = |p: [f32; 3]| -> bool {
            let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() < 1.4
        };
        let base = 3 + total * 9;
        let mut long_edges = 0usize;
        let mut n_tris = 0usize;
        for t in 0..tris {
            let (ia, ib, ic) = (
                out[base + t * 3] as usize,
                out[base + t * 3 + 1] as usize,
                out[base + t * 3 + 2] as usize,
            );
            let (a, b, cc) = (pos(ia), pos(ib), pos(ic));
            if !(near(a) && near(b) && near(cc)) {
                continue;
            }
            n_tris += 1;
            for (x, y, ix, iy) in [(a, b, ia, ib), (b, cc, ib, ic), (cc, a, ic, ia)] {
                let d = [y[0] - x[0], y[1] - x[1], y[2] - x[2]];
                let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let sp = out[3 + ix * 9 + 7].min(out[3 + iy * 9 + 7]).max(0.05);
                if l > 4.0 * sp {
                    long_edges += 1;
                    eprintln!(
                        "LANG {ix}({:?})-{iy}({:?}) l={l:.3} u=({:.2},{:.2}) flags=({},{})",
                        x, y, out[3 + ix * 9 + 7], out[3 + iy * 9 + 7],
                        out[3 + ix * 9 + 8], out[3 + iy * 9 + 8]
                    );
                }
            }
        }
        eprintln!("mini-sphere: {n_tris} tris, {long_edges} lange Kanten (>4×U)");
        // Kernel-Diagnose-Blöcke auslesen (open/weak/long).
        {
            let idx_end = 3 + total * 9 + tris * 3;
            let n_face = out[idx_end] as usize;
            let diag = idx_end + 1 + n_face * 4;
            let n_open = out[diag] as usize;
            let weak_base = diag + 1 + n_open * 2;
            let n_weak = out[weak_base] as usize;
            let long_base = weak_base + 1 + n_weak * 3;
            let n_long = out[long_base] as usize;
            eprintln!("DIAG: open={n_open} weak={n_weak} long={n_long}");
        }
    }

    /// Mess-Matrix des reinen Basis-Algorithmus (ohne Loch-Füller) über
    /// Testkörper × Akzeptanz-Modi — Analyse-Werkzeug für die Prädikat-
    /// Entscheidung (gabriel vs. witness vs. vote). Env LICHEN_MATRIX_CURV.
    #[test]
    #[ignore = "Mess-Matrix (Analyse), kein Gate"]
    fn zz_basis_matrix() {
        let curv: f32 = std::env::var("LICHEN_MATRIX_CURV")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5.0);
        let mk = |name: &str| -> crate::Kernel {
            match name {
                "box" => box_kernel(),
                "cyl" => {
                    let mut k = crate::Kernel::new();
                    k.add_primitive(Primitive::KIND_CYLINDER, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.0, 10.0, 0.0).unwrap();
                    k
                }
                "sphere" => {
                    let mut k = crate::Kernel::new();
                    k.add_primitive(Primitive::KIND_SPHERE, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0).unwrap();
                    k
                }
                "torus" => {
                    let mut k = crate::Kernel::new();
                    k.add_primitive(Primitive::KIND_TORUS, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.0, 3.0, 0.0).unwrap();
                    k
                }
                _ => demo_project_kernel(),
            }
        };
        for name in ["box", "cyl", "sphere", "torus", "demo"] {
            for acc in ["gabriel", "witness", "vote"] {
                let extra = std::env::var("LICHEN_MATRIX_EXTRA").unwrap_or_default();
                let minsp: f32 = std::env::var("LICHEN_MATRIX_MINSP")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.1);
                let mut k = mk(name);
                let h: f32 = std::env::var("LICHEN_MATRIX_H")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(if name == "demo" { 10.0 } else { 3.0 });
                let opts = format!(
                    r#"{{"targetEdgeLength":{h},"grow":true,"curvAngleDeg":{curv},"minEdgeSpacing":{minsp},"maxPoints":50000,"generalTri":true,"accept":"{acc}","fillHoles":false{extra}}}"#
                );
                let _ = &opts;
                let out = k.lichen_mesh(opts);
                let total = out[0] as usize;
                let tris = out[1] as usize;
                let idx_end = 3 + total * 9 + tris * 3;
                let n_face = out[idx_end] as usize;
                let diag = idx_end + 1 + n_face * 4;
                let n_open = out[diag] as usize;
                let weak_base = diag + 1 + n_open * 2;
                let n_weak = out[weak_base] as usize;
                let long_base = weak_base + 1 + n_weak * 3;
                let n_long = out[long_base] as usize;
                let q_base = long_base + 1 + n_long * 2;
                eprintln!(
                    "MATRIX {name:>6} {acc:>7}: pts={total:>5} tris={tris:>5} open={:>4} nm={:>3} orient={:>3} long={n_long:>3} weak={n_weak:>4}",
                    out[q_base] as i64, out[q_base + 1] as i64, out[q_base + 5] as i64
                );
            }
        }
    }

    /// COCONE-SÄULE 3 (Manifold-Extraktion, 2026-07-12): der Sheet-Walk
    /// (Env LICHEN_EXTRACT=walk) liefert auf allen glatten Körpern ein
    /// wasserdichtes 2-Manifold OHNE Kapazitäts-Klemme, OHNE Kandidaten-
    /// Vervollständigung und OHNE Loch-Füller — er ersetzt sie strukturell.
    /// Historie: die Loch-Forensik (LICHEN_WHY) zeigte, dass ~85 % aller
    /// offenen Kanten daran scheiterten, dass eine Nachbarkante bereits von
    /// zwei — ebenfalls kugel-GÜLTIGEN — Dreiecken belegt war; greedy
    /// akzeptieren + Kapazität klemmen ist also strukturell falsch.
    #[test]
    fn lichen_walk_extraction_watertight_on_smooth_bodies() {
        for (name, mk) in [
            ("box", (|| box_kernel()) as fn() -> crate::Kernel),
            ("cyl", || {
                let mut k = crate::Kernel::new();
                k.add_primitive(Primitive::KIND_CYLINDER, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.0, 10.0, 0.0)
                    .unwrap();
                k
            }),
            ("sphere", || {
                let mut k = crate::Kernel::new();
                k.add_primitive(Primitive::KIND_SPHERE, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0)
                    .unwrap();
                k
            }),
        ] {
            let mut k = mk();
            let out = k.lichen_mesh(
                r#"{"targetEdgeLength":3.0,"grow":true,"curvAngleDeg":5.0,"minEdgeSpacing":0.1,"maxPoints":50000,"generalTri":true,"accept":"gabriel","fillHoles":false,"extractWalk":true}"#.into(),
            );
            let (tris, q) = quality_of(&out);
            eprintln!("walk {name}: tris={tris} {q:?}");
            assert!(tris > 100, "{name}: Triangulierung erwartet");
            assert_eq!(q.nonmanifold_edges, 0, "{name}: {q:?}");
            assert_eq!(q.boundary_edges, 0, "{name} watertight OHNE Fallbacks: {q:?}");
            assert_eq!(q.orientation_violations, 0, "{name}: {q:?}");
            assert_eq!(q.degenerate_tris, 0, "{name}: {q:?}");
        }
    }

    /// GLOBALER WALK (Cocone über die GESAMTE Punktwolke, keine Flächen-
    /// Zerlegung): Box, Zylinder, Kugel UND Torus sind ein geschlossenes
    /// 2-Manifold (χ=2) — ohne Chart, ohne Kandidaten-Vervollständigung,
    /// ohne Loch-Füller, ohne Kapazitäts-Klemme. Der Torus (doppelt
    /// periodisch, im Chart-Pfad nie lösbar) fällt hier als Nebenprodukt ab.
    #[test]
    fn lichen_global_walk_watertight_on_smooth_bodies() {
        for (name, mk) in [
            ("box", (|| box_kernel()) as fn() -> crate::Kernel),
            ("cyl", || {
                let mut k = crate::Kernel::new();
                k.add_primitive(Primitive::KIND_CYLINDER, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.0, 10.0, 0.0)
                    .unwrap();
                k
            }),
            ("sphere", || {
                let mut k = crate::Kernel::new();
                k.add_primitive(Primitive::KIND_SPHERE, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0)
                    .unwrap();
                k
            }),
            ("torus", || {
                let mut k = crate::Kernel::new();
                k.add_primitive(Primitive::KIND_TORUS, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, 8.0, 3.0, 0.0)
                    .unwrap();
                k
            }),
        ] {
            let mut k = mk();
            let out = k.lichen_mesh(
                r#"{"targetEdgeLength":3.0,"grow":true,"curvAngleDeg":5.0,"minEdgeSpacing":0.1,"maxPoints":50000,"generalTri":true,"accept":"witness","fillHoles":false,"globalWalk":true}"#.into(),
            );
            let (tris, q) = quality_of(&out);
            eprintln!("global-walk {name}: tris={tris} {q:?}");
            assert!(tris > 100, "{name}: Triangulierung erwartet");
            assert_eq!(q.nonmanifold_edges, 0, "{name}: {q:?}");
            assert_eq!(q.boundary_edges, 0, "{name} watertight OHNE Fallbacks: {q:?}");
            assert_eq!(q.orientation_violations, 0, "{name}: {q:?}");
            assert_eq!(q.degenerate_tris, 0, "{name}: {q:?}");
            // Torus = Genus 1 (χ=0), die übrigen Genus 0 (χ=2).
            let want = if name == "torus" { 0 } else { 2 };
            assert_eq!(q.euler, want, "{name}: Topologie {q:?}");
        }
    }

    /// ε-SAMPLING (beidseitiger Reach, `lfsFactor`): die Abtastung hält einen
    /// Bruchteil der local feature size ein — auch gegen die INNERE Medialachse.
    /// Auf einer dünnen Wand (1 mm) muss das die Punktdichte messbar erhöhen;
    /// ohne diese Bedingung erreicht die leere Umkugel das Gegenblatt (die
    /// Flächen-Zerlegung hat das bisher nur maskiert).
    #[test]
    fn lichen_lfs_sampling_refines_thin_walls() {
        let mk = || {
            let mut k = crate::Kernel::new();
            k.load_json(
                r#"{
                "schema_version": 1,
                "name": "duenne-wand",
                "feature_calls": [
                    { "type": "box", "id": 1, "params": {
                        "op": "new", "half_extents": [10, 10, 0.5], "translate": [0, 0, 0] } }
                ],
                "next_feature_id": 2
            }"#,
            )
            .unwrap();
            k
        };
        let count = |lfs: f32| -> usize {
            let mut k = mk();
            let out = k.lichen_mesh(format!(
                r#"{{"targetEdgeLength":5.0,"grow":true,"curvAngleDeg":20.0,"useReach":true,"reachTol":1.0,"minEdgeSpacing":0.05,"maxPoints":50000,"triangulate":false,"lfsFactor":{lfs}}}"#
            ));
            out[0] as usize
        };
        let (off, on) = (count(0.0), count(0.5));
        eprintln!("dünne Wand (1 mm): Punkte ohne lfs={off}, mit lfs(0.5)={on}");
        assert!(
            on > off * 2,
            "ε-Sampling muss die 1-mm-Wand deutlich feiner abtasten (ohne={off}, mit={on})"
        );

        // FEATURE-PROTECTION (Cheng/Dey/Ramos): an einer scharfen Kante läuft
        // die Medialachse des KÖRPERS bis an die Oberfläche (der Bisektor
        // zweier Nachbarflächen berührt die Kante) — eine naive lfs-Abtastung
        // verdichtet dort ins Unendliche. Der Innen-Marsch zählt deshalb nur
        // echte GEGENBLÄTTER (antiparallele Fußpunkt-Normale). Ein dicker
        // Würfel darf durch ε-Sampling NICHT dichter werden.
        let cube = |lfs: f32| -> usize {
            let mut k = box_kernel();
            let out = k.lichen_mesh(format!(
                r#"{{"targetEdgeLength":3.0,"grow":true,"curvAngleDeg":20.0,"useReach":true,"reachTol":1.0,"minEdgeSpacing":0.1,"maxPoints":50000,"triangulate":false,"lfsFactor":{lfs}}}"#
            ));
            out[0] as usize
        };
        let (c_off, c_on) = (cube(0.0), cube(0.5));
        eprintln!("dicker Würfel: Punkte ohne lfs={c_off}, mit lfs(0.5)={c_on}");
        assert_eq!(
            c_on, c_off,
            "ε-Sampling darf an SCHARFEN KANTEN nicht verdichten (Feature-Protection)"
        );
    }

    /// Analyse: wie viele Punkt-PAARE liegen unzulässig nah beieinander
    /// (Doppelblätter an grazing-Verschneidungen) und wie stehen ihre
    /// Normalen? Unterscheidet Kollaps-Kandidaten (Normalen gleichgerichtet)
    /// von echten Gegenblättern dünner Wände (antiparallel).
    #[test]
    #[ignore = "Analyse-Werkzeug, kein Gate"]
    fn zz_duplicate_sheet_stats() {
        let mut k = demo_project_kernel();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":10.0,"grow":true,"curvAngleDeg":5.0,"minEdgeSpacing":0.1,"maxPoints":50000,"triangulate":false,"lfsFactor":0.5}"#.into(),
        );
        let total = out[0] as usize;
        let p = |i: usize| [out[3 + i * 9], out[3 + i * 9 + 1], out[3 + i * 9 + 2]];
        let n = |i: usize| [out[3 + i * 9 + 4], out[3 + i * 9 + 5], out[3 + i * 9 + 6]];
        let sp = |i: usize| out[3 + i * 9 + 7];
        // Wie viele Punktpaare kommen sich NÄHER als das lokale Spacing?
        // Solche Paare vergiften gegenseitig ihre Umkugeln (jedes Dreieck mit
        // dem einen enthält den anderen) — das ist die Signatur der
        // TANGENTIALEN Annäherung zweier Flächen (lfs → 0), nicht die von
        // Duplikaten.
        for f in [0.5f32, 1.0, 1.5] {
            let (mut same, mut opp) = (0usize, 0usize);
            let mut worst = f32::MAX;
            for i in 0..total {
                for j in i + 1..total {
                    let (a, b) = (p(i), p(j));
                    let d = ((a[0] - b[0]).powi(2)
                        + (a[1] - b[1]).powi(2)
                        + (a[2] - b[2]).powi(2))
                    .sqrt();
                    let s = sp(i).min(sp(j)).max(1e-4);
                    if d < f * s {
                        let (na, nb) = (n(i), n(j));
                        let dp = na[0] * nb[0] + na[1] * nb[1] + na[2] * nb[2];
                        if dp > 0.5 {
                            same += 1;
                            worst = worst.min(d / s);
                        } else {
                            opp += 1;
                        }
                    }
                }
            }
            eprintln!(
                "Punkte={total}: < {f}·space → {same} Paare gleich-orientiert, {opp} gegenläufig (engstes {:.2}·space)",
                if worst.is_finite() { worst } else { 0.0 }
            );
        }
        // Die GEGENBLATT-Paare (< 1·space, gegenläufige Normalen) einzeln:
        // Position, Spacing, Flag — die Wurzel der irreduziblen Front.
        let flag = |i: usize| out[3 + i * 9 + 8];
        let mut shown = 0;
        for i in 0..total {
            for j in i + 1..total {
                let (a, b) = (p(i), p(j));
                let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
                let s = sp(i).min(sp(j)).max(1e-4);
                if d >= s {
                    continue;
                }
                let (na, nb) = (n(i), n(j));
                if na[0] * nb[0] + na[1] * nb[1] + na[2] * nb[2] > -0.3 {
                    continue;
                }
                if shown < 10 {
                    eprintln!(
                        "  GEGENBLATT d={d:.3} | {i} sp={:.2} flag={} at [{:.2},{:.2},{:.2}] | {j} sp={:.2} flag={} at [{:.2},{:.2},{:.2}]",
                        sp(i), flag(i), a[0], a[1], a[2],
                        sp(j), flag(j), b[0], b[1], b[2]
                    );
                    shown += 1;
                }
            }
        }
    }

    /// NORMALEN-GUARD (Betreiber-Idee 2026-07-12) — die dünne Platte ist der
    /// Fall, an dem der Cocone-Filter PRINZIPIELL versagt: bei zwei fast
    /// parallelen Blättern im Abstand t ist die Sehne quer über die Wand zu
    /// einem lateral entfernten Punkt nahezu TANGENTIAL (Winkel ≈ atan(t/L)),
    /// also lässt der Richtungs-Kegel sie durch — erst recht mit dem weiten
    /// Kegel, den der globale Walk für scharfe Kanten braucht. Der Winkel
    /// zwischen den NORMALEN beider Endpunkte ist dort dagegen ~180°.
    /// Kanten-Sonderbehandlung: geprüft wird über die Normalen-MENGE je Punkt
    /// (eine pro angrenzender Fläche), damit die Plattenkante selbst hält.
    #[test]
    fn lichen_thin_plate_global_walk_is_watertight() {
        let mut k = crate::Kernel::new();
        k.load_json(
            r#"{
            "schema_version": 1,
            "name": "duenne-platte",
            "feature_calls": [
                { "type": "box", "id": 1, "params": {
                    "op": "new", "half_extents": [10, 10, 0.5], "translate": [0, 0, 0] } }
            ],
            "next_feature_id": 2
        }"#,
        )
        .unwrap();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":5.0,"grow":true,"curvAngleDeg":5.0,"useReach":true,"reachTol":1.0,"minEdgeSpacing":0.1,"maxPoints":50000,"generalTri":true,"accept":"witness","fillHoles":false,"globalWalk":true,"lfsFactor":0.5,"faceGuard":true}"#.into(),
        );
        let total = out[0] as usize;
        let tris = out[1] as usize;
        assert!(tris > 100, "Triangulierung erwartet");
        let base = 3 + total * 9;
        let n = |i: usize| [out[3 + i * 9 + 4], out[3 + i * 9 + 5], out[3 + i * 9 + 6]];
        // KEIN Dreieck darf die Wand durchqueren: eine Kante zwischen zwei
        // Punkten mit ~antiparallelen Normalen verbindet Vorder- und
        // Rückseite. OHNE den Normalen-Guard passiert genau das (gemessen:
        // χ = −67, also 34 falsche Henkel quer durch die Platte) — der
        // Cocone-Richtungskegel kann es prinzipiell nicht sehen, weil die
        // Sehne bei fast parallelen Blättern nahezu TANGENTIAL liegt.
        let mut through_wall = 0usize;
        for t in 0..tris {
            let idx: Vec<usize> = (0..3).map(|j| out[base + t * 3 + j] as usize).collect();
            for (a, b) in [(idx[0], idx[1]), (idx[1], idx[2]), (idx[2], idx[0])] {
                let (na, nb) = (n(a), n(b));
                if na[0] * nb[0] + na[1] * nb[1] + na[2] * nb[2] < -0.5 {
                    through_wall += 1;
                }
            }
        }
        let (_, q) = quality_of(&out);
        eprintln!("dünne Platte (1 mm), globaler Walk: tris={tris} wand-durchquerungen={through_wall} {q:?}");
        assert_eq!(
            through_wall, 0,
            "Normalen-Guard: keine Kante darf die dünne Wand durchqueren"
        );
        assert_eq!(q.nonmanifold_edges, 0, "{q:?}");
        // WASSERDICHT (seit v0.6.553): die dünne Platte ist der schärfste
        // Fall — Gegenblätter im Abstand 1 mm, eine nur 1 mm breite
        // Seitenfläche und der volle Wächter-Turm. Drei Bedingungen mussten
        // dafür zusammenkommen: Normalen-Guard (Blatt-Trennung), ε-Sampling
        // der KETTEN (die Kante war 7× gröber als die Fläche) und die
        // KETTEN-CLEARANCE im Wachstum (die erste Punktreihe stand auf halbem
        // Abstand vor der Kante und vergiftete die Umkugeln der
        // Seitenflächen-Dreiecke).
        assert_eq!(q.boundary_edges, 0, "watertight OHNE Fallbacks: {q:?}");
        assert_eq!(q.orientation_violations, 0, "{q:?}");
        assert_eq!(q.degenerate_tris, 0, "{q:?}");
        assert_eq!(q.euler, 2, "geschlossene Platte: χ=2, {q:?}");
    }

    /// Betreiber-Punkt #894 (2026-07-12): Loch MITTEN auf der Mini-Kugel-Kappe
    /// — das Wachstum hört dort auf. Diagnose der Wachstums-Ablehnungen und
    /// der Punkt-Verteilung auf der Kugel.
    #[test]
    #[ignore = "Analyse-Werkzeug, kein Gate"]
    fn zz_minisphere_growth_gap() {
        let mut k = demo_project_kernel();
        let out = k.lichen_mesh(
            r#"{"targetEdgeLength":10.0,"grow":true,"curvAngleDeg":5.0,"minEdgeSpacing":0.1,"useReach":true,"reachTol":1.0,"maxPoints":50000,"generalTri":true,"accept":"witness","extractWalk":true,"globalWalk":true,"lfsFactor":0.5,"sizingField":true,"faceGuard":true,"fillHoles":false}"#.into(),
        );
        let total = out[0] as usize;
        let c = [5.2107882f32, 22.11431, 29.24847473977133];
        let pos = |i: usize| [out[3 + i * 9], out[3 + i * 9 + 1], out[3 + i * 9 + 2]];
        // Punkte auf der Mini-Kugel (r=1 ± 0.03).
        let on_sphere: Vec<usize> = (0..total)
            .filter(|&i| {
                let p = pos(i);
                let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
                ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() - 1.0).abs() < 0.03
            })
            .collect();
        eprintln!("Mini-Kugel: {} Punkte von {total}", on_sphere.len());
        // Grösste Lücke: für jeden Kugelpunkt der Abstand zum nächsten Nachbarn.
        let mut worst = (0.0f32, 0usize);
        let mut hist: std::collections::BTreeMap<i32, usize> = Default::default();
        for &i in &on_sphere {
            let p = pos(i);
            let mut nd = f32::MAX;
            for &j in &on_sphere {
                if i == j {
                    continue;
                }
                let q = pos(j);
                let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
                nd = nd.min(d);
            }
            if nd > worst.0 {
                worst = (nd, i);
            }
            *hist.entry((nd * 20.0) as i32).or_default() += 1;
        }
        eprintln!(
            "  grösster Nachbar-Abstand {:.3} bei #{} {:?}; NN-Histogramm (0.05er-Klassen): {hist:?}",
            worst.0, worst.1, pos(worst.1)
        );
        // Deckt die Kappe eine Region ohne Punkte? Richtung [0.35,0.02,0.94]
        // (die Normale von #894) — dort war das Loch.
        let dir = [0.35f32, 0.02, 0.94];
        let probe = [c[0] + dir[0], c[1] + dir[1], c[2] + dir[2]];
        let mut nearest = f32::MAX;
        for &i in &on_sphere {
            let p = pos(i);
            let d = ((p[0] - probe[0]).powi(2) + (p[1] - probe[1]).powi(2) + (p[2] - probe[2]).powi(2)).sqrt();
            nearest = nearest.min(d);
        }
        eprintln!("  Kappen-Mitte {probe:?}: nächster Punkt {:.3} entfernt", nearest);
        // Werden die Kugel-Punkte im NETZ benutzt?
        let tris = out[1] as usize;
        let base = 3 + total * 9;
        let mut used: std::collections::HashSet<usize> = Default::default();
        for t in 0..tris {
            for j in 0..3 {
                used.insert(out[base + t * 3 + j] as usize);
            }
        }
        let unused: Vec<usize> = on_sphere.iter().copied().filter(|i| !used.contains(i)).collect();
        eprintln!(
            "  Kugel-Punkte im Netz benutzt: {}/{} — {} UNBENUTZT",
            on_sphere.len() - unused.len(),
            on_sphere.len(),
            unused.len()
        );
        for &i in unused.iter().take(8) {
            eprintln!("    unbenutzt #{i} {:?} U={:.3}", pos(i), out[3 + i * 9 + 7]);
        }
        // Region um die Kappen-Mitte: wie viele Punkte, wie viele benutzt?
        let near_mid: Vec<usize> = on_sphere
            .iter()
            .copied()
            .filter(|&i| {
                let p = pos(i);
                ((p[0] - probe[0]).powi(2) + (p[1] - probe[1]).powi(2) + (p[2] - probe[2]).powi(2))
                    .sqrt()
                    < 0.35
            })
            .collect();
        let nm_used = near_mid.iter().filter(|i| used.contains(i)).count();
        eprintln!(
            "  Kappen-Mitte (r<0.35): {} Punkte, davon {} im Netz",
            near_mid.len(),
            nm_used
        );
        // Ist die Kappenmitte von Dreiecken ÜBERDECKT?
        let mut cover = 0usize;
        let mut long_near = 0usize;
        for t in 0..tris {
            let idx: Vec<usize> = (0..3).map(|j| out[base + t * 3 + j] as usize).collect();
            let cen = {
                let mut c3 = [0.0f32; 3];
                for &i in &idx {
                    let p = pos(i);
                    for a in 0..3 {
                        c3[a] += p[a] / 3.0;
                    }
                }
                c3
            };
            let dd = ((cen[0] - probe[0]).powi(2) + (cen[1] - probe[1]).powi(2) + (cen[2] - probe[2]).powi(2)).sqrt();
            if dd < 0.25 {
                cover += 1;
                let mut emax = 0.0f32;
                for k in 0..3 {
                    let (a, b) = (pos(idx[k]), pos(idx[(k + 1) % 3]));
                    emax = emax.max(((a[0]-b[0]).powi(2)+(a[1]-b[1]).powi(2)+(a[2]-b[2]).powi(2)).sqrt());
                }
                if emax > 0.3 {
                    long_near += 1;
                }
            }
        }
        eprintln!("  Dreiecke mit Zentroid < 0.25 um die Kappen-Mitte: {cover} (davon {long_near} mit Kante > 0.3)");
        // Offene Kanten irgendwo auf der Mini-Kugel?
        let idx_end = 3 + total * 9 + tris * 3;
        let n_face = out[idx_end] as usize;
        let diag = idx_end + 1 + n_face * 4;
        let n_open = out[diag] as usize;
        let mut open_on_sphere = 0usize;
        for e in 0..n_open {
            let a = out[diag + 1 + e * 2] as usize;
            let b2 = out[diag + 1 + e * 2 + 1] as usize;
            if on_sphere.contains(&a) || on_sphere.contains(&b2) {
                open_on_sphere += 1;
            }
        }
        eprintln!("  offene Kanten gesamt {n_open}, davon an der Mini-Kugel: {open_on_sphere}");
    }

    /// QUALITÄTS-ZIEL auf dem Demo-Projekt: manifold + watertight, beide
    /// Pfade — WIP: (a) Gyroid-Flächen (chartlos, kein FaceSurf) wachsen noch
    /// nicht (F2) → offene Ränder an jeder Gyroid-Naht; (b) Flächen-Überlapp
    /// an Mikro-Feature-Ketten (chart 24 / general 168 non-manifold).
    #[test]
    #[ignore = "WIP: Rest-Löcher an Gyroid-Nähten/Dichte-Übergängen — gemessen \
                2026-07-12 (Chartlos-Fallback + Oberflächen-Gate): chart 15, \
                general 36 offene Kanten (nm/Degeneration/Orientierung sind 0)"]
    fn lichen_demo_project_is_watertight() {
        let mut k = demo_project_kernel();
        for opts in [
            r#"{"targetEdgeLength":10.0,"grow":true,"maxPoints":50000}"#,
            r#"{"targetEdgeLength":10.0,"grow":true,"maxPoints":50000,"generalTri":true}"#,
        ] {
            let out = k.lichen_mesh(opts.into());
            let (_, q) = quality_of(&out);
            assert!(q.watertight(), "{opts}: {q:?}");
        }
    }

    /// Regressionstest Ring-Naht-Kohärenz (Betreiber-Demo OHNE Mikro-
    /// Features): die Gyroid∩Kugel-Naht-Ringe rissen die U(p)-Überdeckungs-
    /// Invariante am Nahtstoß (Mini-Restsegment 0.35 neben 2.82), und ein
    /// Naht-Ring kreuzte im periodischen Kugel-Chart seine Perioden-Kopie
    /// (spade-Panic). Beides gefixt: Naht-Paare erfüllen die Überdeckungs-
    /// regel (Assertion oben läuft mit), kreuzende Constraints werden als
    /// Diagnose übersprungen. Der Lauf selbst ist das Gate.
    #[test]
    fn lichen_gyroid_seam_rings_run_clean() {
        let a = r#"{"schema_version":1,"name":"A","feature_calls":[
            {"type":"sphere","id":1,"params":{"op":"new","radius":50,"translate":[0,0,0]}},
            {"type":"gyroid","id":2,"params":{"op":"intersect","targets":"qCreatedBy(1)",
                "cell_size":{"unit":"mm","value":10},"thickness":{"unit":"mm","value":1},
                "blend_radius":{"unit":"mm","value":0},"chamfer_size":{"unit":"mm","value":0},
                "displace_depth":{"unit":"mm","value":0},"shell_thickness":{"unit":"mm","value":0}}},
            {"type":"cylinder","id":3,"params":{"op":"remove","targets":"qCreatedBy(1)",
                "radius":30,"half_height":60,"translate":[0,0,0]}}
        ],"next_feature_id":10}"#;
        let mut k = crate::Kernel::new();
        k.load_json(a).unwrap();
        let _ = k.lichen_mesh(r#"{"targetEdgeLength":10.0,"grow":true,"maxPoints":50000}"#.into());
    }

    /// BASIS-ALGORITHMUS PUR (Betreiber-„Extrapunkt" 2026-07-11): freie
    /// Kugeln OHNE Rand-Topologie (Seeds aus Feld-Streuung), OHNE Loch-
    /// Füller, Umkugel-Akzeptanz — müssen WASSERDICHT mit Kugel-Topologie
    /// (Euler χ=2) herauskommen. Deckt zugleich den Krümmungs-Sweep ab
    /// (r=1 sub-h, r=8≈h, r=50≫h). Stern-Konsens hat auf denselben Kugeln
    /// 20/42/127 Loch-Kanten (Tangential-Stern-Divergenz) — die Umkugel
    /// ist dort frame-frei eindeutig.
    #[test]
    fn lichen_general_free_spheres_gabriel_watertight_no_fallbacks() {
        for (r, h) in [(1.0f32, 10.0f32), (8.0, 3.0), (50.0, 10.0)] {
            let mut k = crate::Kernel::new();
            k.add_primitive(Primitive::KIND_SPHERE, BoolOp::TAG_NEW, 0.0, 0.0, 0.0, r, 0.0, 0.0)
                .unwrap();
            let out = k.lichen_mesh(format!(
                r#"{{"targetEdgeLength":{h},"grow":true,"maxPoints":50000,"generalTri":true,"accept":"gabriel","fillHoles":false}}"#
            ));
            let (tris, q) = quality_of(&out);
            assert!(tris > 100, "Kugel r={r} muss substanziell trianguliert sein, got {tris}");
            assert!(q.watertight(), "Kugel r={r} nicht wasserdicht: {q:?}");
            assert_eq!(q.euler, 2, "Kugel r={r} hat keine Kugel-Topologie: {q:?}");
            assert_eq!(q.components, 1, "Kugel r={r}: {q:?}");
            assert_eq!(q.orientation_violations, 0, "Kugel r={r}: {q:?}");
        }
    }

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

    /// Ticket 2e7eab20: Überdeckung schlägt min_spacing — eine Mikro-Kante
    /// (Rand-Bedingung 0.017) neben einem 0.3-mm-Segment MUSS auch unterhalb
    /// des Mindestabstands (0.2) gesplittet werden, sonst klafft zwischen den
    /// U-Kreisen eine Lücke: min(dl,d)+min(d,dr) ≥ d gilt danach überall.
    #[test]
    fn coverage_split_beats_min_spacing_at_micro_corners() {
        let line = vec![[0.0f32, 0.0, 0.0], [0.316, 0.0, 0.0]];
        let out = crate::lichen::resample_edge_by_curvature(
            &line,
            10.0,
            5.0f32.to_radians(),
            0.2,
            (0.017, 0.024),
        );
        assert!(out.len() > 2, "Coverage-Split muss Punkte einfügen, got {}", out.len());
        // Segment-Kette inkl. virtueller Rand-Nachbarn: überall überdeckt.
        let mut segs: Vec<f32> = vec![0.017];
        for w in out.windows(2) {
            segs.push(w[1][0] - w[0][0]);
        }
        segs.push(0.024);
        for k in 1..segs.len() - 1 {
            let (dl, d, dr) = (segs[k - 1], segs[k], segs[k + 1]);
            assert!(
                dl.min(d) + dr.min(d) >= d * 0.999,
                "Lücke bei Segment {k}: dl={dl:.4} d={d:.4} dr={dr:.4}"
            );
        }
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



