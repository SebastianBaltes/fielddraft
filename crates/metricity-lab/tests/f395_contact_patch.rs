//! F2/#395 — Prototyp & Messung: Korrekturkörper für submergierte koplanare
//! Kontaktflächen (Design-/Mess-Spike, KEIN Produktions-Opcode).
//!
//! Kontext: Die offset_tree-Pipeline (#384) lässt an einem `−δ`-über-Union-Stopp
//! den naiven Körper `min(f_A, f_B) − δ` stehen und korrigiert die transversalen
//! Nahtkurven mit 1D-Arc-Spline-Kanälen (`offset_channel.rs`). Bei KOPLANAREM
//! Kontakt (bündig gestapelte Boxen) existiert keine transversale Naht — die
//! gemeinsame Grenzfläche ist ein flächiger 2D-Bereich auf einer Ebene Π, und
//! die nächste Primitivfläche ist SUBMERGIERT (liegt im Inneren des Nachbarn).
//! `min` unterschätzt dort die Tiefe, die Erosions-Nullmenge liegt bis > 13 mm
//! neben der wahren Offset-Fläche (Befund F2). Seit v0.18.2 meldet der Kernel
//! das ehrlich als `conservative`.
//!
//! Dieser Spike prüft den hergeleiteten Korrekturterm rein in metricity-lab
//! (f64, keine Kernel-Änderung):
//!
//!   f_true(p) = f_U_exact(p) + |δ|        (Erosion, Minkowski: X⊖δ hat SDF f_X+|δ|)
//!   f_naiv(p) = min(f_A, f_B) + |δ|        (heutiger Stopp)
//!   Fehler   = f_U_exact − min(f_A, f_B)   (Innen-SDF-Fehler der submerg. Fläche)
//!
//! Kandidat (a) — Kontakt-Prisma: der gemeinsame Säulenkörper S über dem
//! Kontaktpolygon P auf Π (Extrude von P zwischen den Außendeckeln beider
//! Boxen). S ⊆ A∪B, hat KEINE innere Fläche auf Π, also
//!   f_korr(p) = min(f_A, f_B, f_S) + |δ|
//! füllt den Phantom-Schlitz. Für gleiche Querschnitte ist S = A∪B → exakt.
//! Rest an ∂P (Stufenkante bei ungleichen Querschnitten) ist der bekannte
//! konkave B0-Sliver (δ·(1/sin(α/2)−1)), den ein Arc-Kanal ENTLANG ∂P schließt.
//!
//! Kandidat (b) — nur 1D-Arc-Kanal entlang ∂P: kann den FLÄCHIGEN Innenfehler
//! (bis zur Mitte von P, ≫ δ von ∂P entfernt) prinzipiell nicht abdecken
//! (Trust-Radius ~c·δ). Hier als Gegenprobe mitgemessen.
//!
//! Lauf: `cargo test -p metricity-lab --test f395_contact_patch -- --nocapture`
//! (Langläufe `#[ignore]`, mit `--ignored` dazuschalten).

use rayon::prelude::*;

use metricity_lab::field::{Field, FnField};
use metricity_lab::ground_truth::{GroundTruth, GtOptions};
use metricity_lab::util::SplitMix64;

// ─── f64-Vektor-/Quaternion-Helfer ────────────────────────────────────────

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] { [a[0]-b[0], a[1]-b[1], a[2]-b[2]] }
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] { [a[0]+b[0], a[1]+b[1], a[2]+b[2]] }
fn scl(a: [f64; 3], s: f64) -> [f64; 3] { [a[0]*s, a[1]*s, a[2]*s] }
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 { a[0]*b[0]+a[1]*b[1]+a[2]*b[2] }
fn norm(a: [f64; 3]) -> f64 { dot(a, a).sqrt() }
fn nrm(a: [f64; 3]) -> [f64; 3] { let n = norm(a).max(1e-30); [a[0]/n, a[1]/n, a[2]/n] }

/// Rotiert `v` mit Einheitsquaternion `q = (x,y,z,w)`.
fn qrot(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let u = [x, y, z];
    let uv = [u[1]*v[2]-u[2]*v[1], u[2]*v[0]-u[0]*v[2], u[0]*v[1]-u[1]*v[0]];
    let uuv = [u[1]*uv[2]-u[2]*uv[1], u[2]*uv[0]-u[0]*uv[2], u[0]*uv[1]-u[1]*uv[0]];
    [
        v[0] + 2.0*(w*uv[0] + uuv[0]),
        v[1] + 2.0*(w*uv[1] + uuv[1]),
        v[2] + 2.0*(w*uv[2] + uuv[2]),
    ]
}

fn quat_axis_angle(axis: [f64; 3], ang: f64) -> [f64; 4] {
    let a = nrm(axis);
    let s = (ang*0.5).sin();
    [a[0]*s, a[1]*s, a[2]*s, (ang*0.5).cos()]
}

// ─── Orientierte Box (exaktes f64-SDF) ─────────────────────────────────────

#[derive(Clone, Copy)]
struct OBox {
    c: [f64; 3],
    h: [f64; 3],
    axes: [[f64; 3]; 3], // orthonormale Spalten (Welt-Achsen der Box)
}

impl OBox {
    fn aa(c: [f64; 3], h: [f64; 3]) -> Self {
        OBox { c, h, axes: [[1.0,0.0,0.0],[0.0,1.0,0.0],[0.0,0.0,1.0]] }
    }
    fn rotated(self, q: [f64; 4]) -> Self {
        OBox {
            c: qrot(q, self.c),
            h: self.h,
            axes: [qrot(q, self.axes[0]), qrot(q, self.axes[1]), qrot(q, self.axes[2])],
        }
    }
    fn sdf(&self, p: [f64; 3]) -> f64 {
        let d = sub(p, self.c);
        let l = [dot(d, self.axes[0]), dot(d, self.axes[1]), dot(d, self.axes[2])];
        let q = [l[0].abs()-self.h[0], l[1].abs()-self.h[1], l[2].abs()-self.h[2]];
        let outside = norm([q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)]);
        let inside = q[0].max(q[1]).max(q[2]).min(0.0);
        outside + inside
    }
    /// AABB in Weltkoordinaten.
    fn aabb(&self) -> ([f64; 3], [f64; 3]) {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for sx in [-1.0, 1.0] { for sy in [-1.0, 1.0] { for sz in [-1.0, 1.0] {
            let corner = add(self.c, add(add(
                scl(self.axes[0], sx*self.h[0]),
                scl(self.axes[1], sy*self.h[1])),
                scl(self.axes[2], sz*self.h[2])));
            for k in 0..3 { lo[k] = lo[k].min(corner[k]); hi[k] = hi[k].max(corner[k]); }
        }}}
        (lo, hi)
    }
}

// ─── Korrektur-Datenstrukturen ─────────────────────────────────────────────

/// Kontakt-Prisma = orientierte Box über dem Kontaktpolygon.
type Prism = OBox;

/// Rim-Kanal: geradliniges Kantensegment auf ∂P + duale Kegelvektoren
/// (konkav, negiert gespeichert wie im Kernel). f64-Port von
/// `offset_channel::channel_apply` (konkaver Zweig, Liniensegment).
#[derive(Clone, Copy)]
struct RimChannel {
    p0: [f64; 3],
    p1: [f64; 3],
    m1: [f64; 3], // duale Kegelvektoren (negiert)
    m2: [f64; 3],
    delta: f64,   // |δ|
    trust: f64,
    gate: f64,
}

impl RimChannel {
    fn closest(&self, p: [f64; 3]) -> (f64, [f64; 3]) {
        let d = sub(self.p1, self.p0);
        let len2 = dot(d, d).max(1e-30);
        let t = (dot(sub(p, self.p0), d) / len2).clamp(0.0, 1.0);
        let q = add(self.p0, scl(d, t));
        (norm(sub(p, q)), q)
    }
    /// Wendet den konkaven Kanal auf den naiven Wert `d` an (Material addieren).
    fn apply(&self, d: f64, p: [f64; 3]) -> f64 {
        let (de, q) = self.closest(p);
        if self.trust - de <= -d { return d; } // Fernfeld-Skip
        let dq = sub(p, q);
        let s1 = dot(dq, self.m1);
        let s2 = dot(dq, self.m2);
        let gate = self.gate - d;
        let w_geo = s1.min(s2).min(de - self.delta).min(self.trust - de);
        let w = w_geo.min(gate);
        if w > 0.0 { d.min(-w) } else if w_geo <= 0.0 { d.min(-w_geo) } else { d }
    }
}

/// Konkave Vertex-Kugel an einer Kontaktrechteck-Ecke (Triple-Junction).
/// f64-Port von `offset_channel::vertex_apply` (konkaver Zweig). Wird synthetisiert
/// (Ecken-Zählung im Report), aber im Feld NICHT angewandt: der f64-Port ist
/// gitter-/δ-empfindlich; maßgeblich ist die EXAKTE Kernel-C4-Vertex-Kugel.
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct CornerSphere {
    v: [f64; 3],
    gates: [[f64; 3]; 3], // Kegel-Facetten-Normalen (negiert)
    delta: f64,
    trust: f64,
    gate: f64,
}

#[allow(dead_code)]
impl CornerSphere {
    fn apply(&self, d: f64, p: [f64; 3]) -> f64 {
        let dq = sub(p, self.v);
        let rho = norm(dq);
        if self.trust - rho <= -d { return d; }
        let s1 = dot(dq, self.gates[0]);
        let s2 = dot(dq, self.gates[1]);
        let s3 = dot(dq, self.gates[2]);
        let gate = self.gate - d;
        // Nur additiv (Zero-Set): der march-sichere Luftseiten-Zweig
        // (d.min(-w_geo)) des Kernels ist für die Nullmenge nicht nötig und
        // im f64-Prototyp orientierungs-empfindlich → weggelassen.
        let w = s1.min(s2).min(s3).min(rho - self.delta).min(self.trust - rho).min(gate);
        if w > 0.0 { d.min(-w) } else { d }
    }
}

struct Correction {
    prisms: Vec<Prism>,
    rims: Vec<RimChannel>,
    corners: Vec<CornerSphere>,
}

// ─── Kontakt-Erkennung + Korrektur-Synthese (f64-Spiegel des Designs) ──────

/// Eine Box-Fläche in Weltkoordinaten.
struct Face {
    n: [f64; 3],       // Außennormale
    o: f64,            // Ebenen-Offset n·x = o
    center: [f64; 3],
    e1: [f64; 3], h1: f64, // In-Ebenen-Achse 1 + Halblänge
    e2: [f64; 3], h2: f64,
    depth: f64,        // volle Box-Dicke normal zur Fläche (2·h_normal)
}

fn box_faces(b: &OBox) -> Vec<Face> {
    let mut out = Vec::new();
    for k in 0..3 {
        let (a, c) = ((k+1)%3, (k+2)%3);
        for s in [-1.0, 1.0] {
            let n = scl(b.axes[k], s);
            let center = add(b.c, scl(n, b.h[k]));
            out.push(Face {
                n, o: dot(n, center), center,
                e1: b.axes[a], h1: b.h[a],
                e2: b.axes[c], h2: b.h[c],
                depth: 2.0 * b.h[k],
            });
        }
    }
    out
}

/// Baut Kontakt-Prismen + Rim-Kanäle für koplanare, gegenläufige, überlappende
/// Flächenpaare zweier Union-Member. `delta` > 0 (|δ|). Findet KEINE Korrektur
/// bei transversalen (Dieder) oder disjunkten Membern → Feld bleibt naiv.
fn build_correction(members: &[OBox], delta: f64) -> Correction {
    let mut prisms = Vec::new();
    let mut rims = Vec::new();
    let mut corners = Vec::new();
    let coincide_tol = (0.02 * delta).max(1e-4);
    let overlap_min = (0.1 * delta).max(1e-3);

    for i in 0..members.len() {
        for j in (i+1)..members.len() {
            let fi = box_faces(&members[i]);
            let fj = box_faces(&members[j]);
            for a in &fi {
                for b in &fj {
                    // Anti-parallel + koinzident.
                    if dot(a.n, b.n) > -0.9995 { continue; }
                    if (a.o + b.o).abs() > coincide_tol { continue; }
                    // In-Ebenen-Basen matchen (koplanare Boxen: gleiche Orient.).
                    let (ea1, ha1) = (a.e1, a.h1);
                    let (ea2, ha2) = (a.e2, a.h2);
                    // b-Halbextente auf a-Basis projizieren.
                    let bh1 = b.h1 * dot(b.e1, ea1).abs() + b.h2 * dot(b.e2, ea1).abs();
                    let bh2 = b.h1 * dot(b.e1, ea2).abs() + b.h2 * dot(b.e2, ea2).abs();
                    // Zentren-Versatz in der Ebene (a-Basis).
                    let dc = sub(b.center, a.center);
                    let off1 = dot(dc, ea1);
                    let off2 = dot(dc, ea2);
                    // Kontakt-Intervalle je Achse: [a: −ha..ha] ∩ [b: off±bh].
                    let lo1 = (-ha1).max(off1 - bh1);
                    let hi1 = ha1.min(off1 + bh1);
                    let lo2 = (-ha2).max(off2 - bh2);
                    let hi2 = ha2.min(off2 + bh2);
                    if hi1 - lo1 < overlap_min || hi2 - lo2 < overlap_min { continue; }
                    let rh1 = 0.5 * (hi1 - lo1);
                    let rh2 = 0.5 * (hi2 - lo2);
                    let rc = add(a.center, add(scl(ea1, 0.5*(lo1+hi1)), scl(ea2, 0.5*(lo2+hi2))));

                    // Prisma: Querschnitt = Kontaktrechteck, Normalachse spannt
                    // beide Box-Tiefen. N = a.n (aus Box i heraus). Box i liegt
                    // auf −N, Box j auf +N (b.n = −a.n).
                    let n = a.n;
                    let half_n = 0.5 * (a.depth + b.depth);
                    let center = add(rc, scl(n, 0.5*(b.depth - a.depth)));
                    prisms.push(OBox { c: center, h: [rh1, rh2, half_n], axes: [ea1, ea2, n] });

                    // Rim-Kanäle an den vier Kanten des Kontaktrechtecks, wo
                    // eine Stufe existiert. Je Kante: die bindende (kürzere) Box
                    // stellt die Wand, die längere Box die Kontaktfläche der Naht.
                    // (Vorzeichen s, Wand-Achse, Grenze-a, Grenze-b, andere Achse,
                    //  anderes-Intervall).
                    let edge_specs: [(f64, [f64;3], f64, f64, [f64;3], f64, f64); 4] = [
                        ( 1.0, ea1,  ha1, off1 + bh1, ea2, lo2, hi2), // Achse1 oben
                        (-1.0, ea1, -ha1, off1 - bh1, ea2, lo2, hi2), // Achse1 unten
                        ( 1.0, ea2,  ha2, off2 + bh2, ea1, lo1, hi1), // Achse2 oben
                        (-1.0, ea2, -ha2, off2 - bh2, ea1, lo1, hi1), // Achse2 unten
                    ];
                    // Alle Stufen je Achse (bis zu 2: ober/unter) sammeln.
                    let mut edges_axis: [Vec<(f64, [f64;3])>; 2] = [Vec::new(), Vec::new()];
                    for (ai, &(s, wall_axis, cand_a, cand_b, other_axis, olo, ohi)) in edge_specs.iter().enumerate() {
                        let axis_idx = ai / 2; // 0,0,1,1
                        // Bindende Grenze = die dem Inneren nähere (min bei s>0,
                        // max bei s<0). Stufe existiert nur bei echtem Unterschied.
                        if (cand_a - cand_b).abs() < coincide_tol { continue; }
                        let coord = if s > 0.0 { cand_a.min(cand_b) } else { cand_a.max(cand_b) };
                        let a_binds = (cand_a - coord).abs() < (cand_b - coord).abs();
                        // n1 = Wand der kürzeren Box (außen = s·wall_axis).
                        let n1 = scl(wall_axis, s);
                        // n2 = Kontaktnormale der LÄNGEREN Box, zeigt zur kürzeren
                        // Seite: a bindet ⇒ b länger ⇒ n2 = b.n = −n; sonst +n.
                        let n2 = if a_binds { scl(n, -1.0) } else { n };
                        let cc = dot(n1, n2);
                        let m1 = sub(n1, scl(n2, cc));
                        let m2 = sub(n2, scl(n1, cc));
                        if norm(m1) < 1e-4 || norm(m2) < 1e-4 { continue; }
                        let m1 = scl(nrm(m1), -1.0);
                        let m2 = scl(nrm(m2), -1.0);
                        let sin_half = ((1.0 - cc) * 0.5).sqrt().max(1e-3);
                        let cfac = 1.0 / sin_half;
                        let trust = cfac * delta + 4.0e-3;
                        let gate = (cfac - 1.0) * delta + 4.0e-3;
                        // Kantensegment auf der Ebene: fix in wall_axis (coord ab
                        // a.center), spannt other_axis über [olo, ohi].
                        let base = add(a.center, scl(wall_axis, coord));
                        let p0 = add(base, scl(other_axis, olo));
                        let p1 = add(base, scl(other_axis, ohi));
                        rims.push(RimChannel { p0, p1, m1, m2, delta, trust, gate });
                        edges_axis[axis_idx].push((coord, scl(wall_axis, s)));
                    }
                    // Ecken = Triple-Junctions, wo eine Achse-0- und eine Achse-1-
                    // Stufe zusammenlaufen (bis zu 4). Dritte Gate-Normale
                    // (Kontaktfläche) per Feld-Probe der Luftseite bestimmt.
                    for &(c0, wn0) in &edges_axis[0] {
                        for &(c1, wn1) in &edges_axis[1] {
                            let v = add(a.center, add(scl(ea1, c0), scl(ea2, c1)));
                            let lat = add(wn0, wn1); // Luft-Richtung lateral
                            let deep = add(v, scl(lat, 0.3 * delta));
                            let bp = min_members(members, add(deep, scl(n, 0.3 * delta)));
                            let bm = min_members(members, sub(deep, scl(n, 0.3 * delta)));
                            let n3 = if bp > 0.0 && bm < 0.0 { n }
                                     else if bm > 0.0 && bp < 0.0 { scl(n, -1.0) }
                                     else { [0.0; 3] };
                            if norm(n3) < 0.5 { continue; }
                            let gates = [scl(wn0, -1.0), scl(wn1, -1.0), scl(n3, -1.0)];
                            let cfac = 3.0f64.sqrt(); // orthogonale Ecke: c_v ≈ √3
                            corners.push(CornerSphere {
                                v, gates, delta,
                                trust: cfac * delta + 4.0e-3,
                                gate: (cfac - 1.0) * delta + 4.0e-3,
                            });
                        }
                    }
                }
            }
        }
    }
    Correction { prisms, rims, corners }
}

// ─── Felder ────────────────────────────────────────────────────────────────

fn min_members(members: &[OBox], p: [f64; 3]) -> f64 {
    members.iter().map(|m| m.sdf(p)).fold(f64::INFINITY, f64::min)
}

// ─── Messung (Nullmengen-Abweichung gegen die wahre Offset-Fläche) ─────────

struct Zs { p99: f64, max: f64, mean: f64, n: usize }

/// Misst |d_base(p) − |δ|| über die Oberflächen-Punktwolke von `field`.
/// `base_gt` = Ground Truth des Grundkörpers (Union), liefert d_base.
fn measure(
    field: &dyn Field,
    base_gt: &GroundTruth,
    lo: [f64; 3], hi: [f64; 3],
    delta_abs: f64,
    grid: usize, n_probe: usize, seed: u64,
) -> Option<Zs> {
    let opts = GtOptions { grid, ..GtOptions::default() };
    let gt = GroundTruth::build(field, lo, hi, &opts);
    let surf = gt.surface_points();
    if surf.is_empty() { return None; }
    let mut rng = SplitMix64::new(seed ^ 0x9E37_79B9);
    let probe: Vec<[f64;3]> = (0..n_probe.min(surf.len()))
        .map(|_| surf[(rng.next_u64() as usize) % surf.len()]).collect();
    let mut errs: Vec<f64> = probe.par_iter()
        .map(|&p| (base_gt.distance(p).abs() - delta_abs).abs())
        .filter(|e| e.is_finite()).collect();
    if errs.is_empty() { return None; }
    let n = errs.len();
    let mean = errs.iter().sum::<f64>() / n as f64;
    errs.sort_by(|a,b| a.partial_cmp(b).unwrap());
    let max = *errs.last().unwrap();
    let p99 = errs[((n as f64*0.99) as usize).min(n-1)];
    Some(Zs { p99, max, mean, n })
}

fn scene_aabb(members: &[OBox]) -> ([f64;3],[f64;3]) {
    let mut lo = [f64::INFINITY;3];
    let mut hi = [f64::NEG_INFINITY;3];
    for m in members {
        let (l,h) = m.aabb();
        for k in 0..3 { lo[k]=lo[k].min(l[k]); hi[k]=hi[k].max(h[k]); }
    }
    (lo, hi)
}

// ─── Szenen ────────────────────────────────────────────────────────────────

struct SceneDef {
    name: &'static str,
    members: Vec<OBox>,
    delta: f64, // negativ = Erosion
    /// Korrektur greift (koplanarer Kontakt) — erwartet der Test.
    correctable: bool,
    /// Reiner koplanarer Stapel-Kontakt (F2-Repro): Prisma + Rim reichen, keine
    /// Überhang-Triple-Junctions (die brauchen C4-Vertex-Kugeln, Kernel).
    pure: bool,
}

fn scenes() -> Vec<SceneDef> {
    let mut v = Vec::new();
    // 1. Koplanar, GLEICHER Querschnitt → Union ist eine größere Box (prism exakt).
    v.push(SceneDef {
        name: "coplanar_equal",
        members: vec![
            OBox::aa([0.0,0.0,0.0],[15.0,20.0,12.0]),
            OBox::aa([27.0,0.0,0.0],[12.0,20.0,12.0]),
        ],
        delta: -2.0, correctable: true,
        pure: true,
    });
    // 2. Koplanar, UNGLEICHER Querschnitt (F2-Repro, fuzz coplanar-boxes) → Stufe.
    v.push(SceneDef {
        name: "coplanar_step",
        members: vec![
            OBox::aa([0.0,0.0,0.0],[15.0,22.0,12.0]),
            OBox::aa([29.0,0.0,0.0],[14.0,16.0,18.0]),
        ],
        delta: -2.0, correctable: true,
        pure: true,
    });
    // 3. T-Stoß: schmaler Steg bündig auf breiter Platte (Kontaktpolygon = Steg).
    v.push(SceneDef {
        name: "t_joint",
        members: vec![
            OBox::aa([0.0,0.0,0.0],[40.0,40.0,6.0]),          // Platte
            OBox::aa([0.0,0.0,6.0+14.0],[8.0,8.0,14.0]),       // Steg (z bündig)
        ],
        delta: -2.0, correctable: true,
        pure: false,
    });
    // 4. Teilüberlappende Boxen: Kontaktpolygon = echtes Schnitt-Sub-Rechteck.
    v.push(SceneDef {
        name: "partial_overlap",
        members: vec![
            OBox::aa([0.0,0.0,0.0],[18.0,18.0,12.0]),
            OBox::aa([30.0,7.0,3.0],[12.0,14.0,16.0]),         // in y,z versetzt
        ],
        delta: -2.0, correctable: true,
        pure: false,
    });
    // 5. Rotiert: coplanar_step rigide gedreht (Naht nicht achsparallel).
    {
        let q = quat_axis_angle([0.3,1.0,0.5], 0.7);
        v.push(SceneDef {
            name: "coplanar_step_rot",
            members: vec![
                OBox::aa([0.0,0.0,0.0],[15.0,22.0,12.0]).rotated(q),
                OBox::aa([29.0,0.0,0.0],[14.0,16.0,18.0]).rotated(q),
            ],
            delta: -2.0, correctable: true,
            pure: true,
        });
    }
    // ─ Gegenproben (Korrektur darf NICHT feuern / nicht verschlechtern) ─
    // 6. Transversaler Dieder (RotatedUnionOffset-Klasse).
    {
        let q = quat_axis_angle([0.0,0.0,1.0], 50f64.to_radians());
        v.push(SceneDef {
            name: "dihedral_transversal",
            members: vec![
                OBox::aa([0.0,-30.0,0.0],[60.0,30.0,20.0]),
                OBox::aa([0.0,-30.0,0.0],[60.0,30.0,20.0]).rotated(q),
            ],
            delta: -2.0, correctable: false,
            pure: false,
        });
    }
    // 7. Disjunkt.
    v.push(SceneDef {
        name: "disjoint",
        members: vec![
            OBox::aa([0.0,0.0,0.0],[12.0,12.0,12.0]),
            OBox::aa([40.0,0.0,0.0],[12.0,12.0,12.0]),
        ],
        delta: -2.0, correctable: false,
        pure: false,
    });
    v
}

// ─── Der Mess-Lauf ─────────────────────────────────────────────────────────

fn run_scene(s: &SceneDef, grid: usize, probe: usize) -> String {
    let da = s.delta.abs();
    let (lo, hi) = scene_aabb(&s.members);
    let pad = 0.1 * (hi[0]-lo[0]).max(hi[1]-lo[1]).max(hi[2]-lo[2]);
    let lo = [lo[0]-pad, lo[1]-pad, lo[2]-pad];
    let hi = [hi[0]+pad, hi[1]+pad, hi[2]+pad];

    let members = s.members.clone();
    let base = FnField(move |p| min_members(&members, p));
    let base_gt = GroundTruth::build(&base, lo, hi, &GtOptions { grid, ..GtOptions::default() });

    let corr = build_correction(&s.members, da);

    let m1 = s.members.clone();
    let d = s.delta;
    let naive = FnField(move |p| min_members(&m1, p) - d);

    let m2 = s.members.clone();
    let prisms = corr.prisms.clone();
    let dd = s.delta;
    let prism_field = FnField(move |p| {
        let mut v = min_members(&m2, p);
        for pr in &prisms { v = v.min(pr.sdf(p)); }
        v - dd
    });

    let m3 = s.members.clone();
    let prisms3 = corr.prisms.clone();
    let rims3 = corr.rims.clone();
    let d3 = s.delta;
    let full_field = FnField(move |p| {
        let mut v = min_members(&m3, p);
        for pr in &prisms3 { v = v.min(pr.sdf(p)); }
        let mut f = v - d3;
        for r in &rims3 { f = r.apply(f, p); }
        // corners: siehe C4-Vertex-Kugeln (Kernel); Prototyp-Port imperfekt, hier aus
        f
    });

    let z_naive = measure(&naive, &base_gt, lo, hi, da, grid, probe, 1);
    let z_prism = measure(&prism_field, &base_gt, lo, hi, da, grid, probe, 2);
    let z_full  = measure(&full_field, &base_gt, lo, hi, da, grid, probe, 3);

    let fmt = |z: &Option<Zs>| match z {
        Some(z) => format!("p99={:.4} max={:.4} mean={:.4} n={}", z.p99, z.max, z.mean, z.n),
        None => "KOLLAPS".to_string(),
    };
    format!(
        "[{:>20}] δ={:+.1} prism={} rim={} corner={} | naiv:  {}\n{:>24} | prism: {}\n{:>24} | full:  {}",
        s.name, s.delta, corr.prisms.len(), corr.rims.len(), corr.corners.len(),
        fmt(&z_naive), "", fmt(&z_prism), "", fmt(&z_full),
    )
}

#[test]
fn f395_contact_patch_report() {
    // CI-tauglich: moderates Gitter. Voller Sweep in `#[ignore]`.
    let grid = 56;
    let probe = 300;
    eprintln!("=== F395 Kontakt-Prisma-Spike (grid={grid}, probe={probe}) ===");
    let mut worst_pure_full_p99 = 0.0f64;
    for s in scenes() {
        let da = s.delta.abs();
        let (lo, hi) = scene_aabb(&s.members);
        let pad = 0.1 * (hi[0]-lo[0]).max(hi[1]-lo[1]).max(hi[2]-lo[2]);
        let lo = [lo[0]-pad, lo[1]-pad, lo[2]-pad];
        let hi = [hi[0]+pad, hi[1]+pad, hi[2]+pad];
        let members = s.members.clone();
        let base = FnField(move |p| min_members(&members, p));
        let base_gt = GroundTruth::build(&base, lo, hi, &GtOptions { grid, ..GtOptions::default() });
        let corr = build_correction(&s.members, da);

        let m1 = s.members.clone(); let d = s.delta;
        let naive = FnField(move |p| min_members(&m1, p) - d);
        let m2 = s.members.clone(); let prisms2 = corr.prisms.clone(); let d2 = s.delta;
        let prism = FnField(move |p| {
            let mut v = min_members(&m2, p);
            for pr in &prisms2 { v = v.min(pr.sdf(p)); }
            v - d2
        });
        let m3 = s.members.clone(); let prisms3 = corr.prisms.clone();
        let rims3 = corr.rims.clone(); let d3 = s.delta;
        let full = FnField(move |p| {
            let mut v = min_members(&m3, p);
            for pr in &prisms3 { v = v.min(pr.sdf(p)); }
            let mut f = v - d3;
            for r in &rims3 { f = r.apply(f, p); }
            // corners: siehe C4-Vertex-Kugeln (Kernel); Prototyp-Port imperfekt, hier aus
            f
        });
        let zn = measure(&naive, &base_gt, lo, hi, da, grid, probe, 1);
        let zp = measure(&prism, &base_gt, lo, hi, da, grid, probe, 2);
        let zf = measure(&full, &base_gt, lo, hi, da, grid, probe, 3);
        eprintln!("{}", run_scene(&s, grid, probe));

        if s.correctable {
            // Kern-Property: das Prisma tilgt den FLÄCHIGEN 2D-Innenfehler.
            // Der mittlere Fehler muss um Größenordnungen fallen (der naive
            // Phantom-Schlitz ist verteilt, nicht nur ein Randeffekt).
            if let (Some(n), Some(p)) = (&zn, &zp) {
                assert!(p.mean < n.mean * 0.25 + 1e-6,
                    "{}: Prisma tilgt den 2D-Innenfehler nicht (mean {:.4} → {:.4})",
                    s.name, n.mean, p.mean);
                assert!(p.p99 < 1.0,
                    "{}: Prisma-p99 zu hoch ({:.4}) — Innenfehler nicht getilgt", s.name, p.p99);
            }
            if s.pure {
                // Reiner koplanarer Stapel (F2-Repro): Prisma + ∂P-Rim-Kanal
                // erreichen die Zielmarke. Bei δ=1/2 mm liegt der Restfehler in
                // Kanal-Toleranz-Ordnung (0.001–0.018 mm gemessen); der f64-Rim-
                // Port ist gitter-/δ-empfindlich (der EXAKTE Kanal-Gate des
                // Kernels ist maßgeblich), daher gaten wir konservativ < 0.3 mm —
                // immer noch > 30× unter dem naiven Fehler (> 9 mm).
                if let Some(z) = &zf {
                    worst_pure_full_p99 = worst_pure_full_p99.max(z.p99);
                    assert!(z.p99 < 0.3,
                        "{}: Prisma+Rim erreicht Zielmarke nicht (p99 {:.4})", s.name, z.p99);
                }
            }
        } else {
            // Gegenprobe: keine Korrektur synthetisiert (kein koplanarer Kontakt).
            assert!(corr.prisms.is_empty() && corr.rims.is_empty(),
                "{}: Korrektur feuert fälschlich (prism={}, rim={})",
                s.name, corr.prisms.len(), corr.rims.len());
        }
    }
    eprintln!("worst PURE koplanar full p99 (nachher): {worst_pure_full_p99:.4} mm");
}

/// δ-Sweep: bestätigt, dass der Prism-Rest an ∂P der B0-Sliver δ·(1/sin(α/2)−1)
/// ist (skaliert mit δ) und der Rim-Kanal ihn schließt.
#[test]
#[ignore]
fn f395_delta_sweep() {
    let grid = 72;
    let probe = 400;
    eprintln!("=== F395 δ-Sweep (coplanar_step, T-Stoß) ===");
    for name in ["coplanar_step", "t_joint"] {
        for &delta in &[-1.0f64, -2.0, -4.0] {
            let mut s = scenes().into_iter().find(|x| x.name == name).unwrap();
            s.delta = delta;
            eprintln!("{}", run_scene(&s, grid, probe));
        }
    }
}
