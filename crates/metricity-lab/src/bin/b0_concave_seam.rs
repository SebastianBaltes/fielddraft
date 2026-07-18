//! B0-Messreihe „konkave Shell-Naht" (#384, offset_tree Phase B).
//!
//! Der Spike (iter3) hat nur den KONVEXEN Kanal-Fall vermessen
//! (Überschuss-Keil bei +δ über Schnitt). Diese Messreihe vermisst den
//! bislang ungemessenen KONKAVEN Fall: Wie falsch ist das heutige Feld
//! (nach Pushdown + Blatt-Einfalten, exakt der Baum, den Phase B
//! vorfindet) an konkaven Nähten unter Erosion/Innen-Offset — und ist
//! es dort **nullmengen-exakt** (dann braucht Phase B dort keinen
//! Korrekturkörper) oder gibt es echte Zero-Set-/Vorzeichenfehler?
//!
//! Fixtures (alle durch die Produktions-Normalisierung
//! `push_down_offsets().fold_offset_leaves()` — identisch zu
//! `body.rs::build_bounded_for_body_rounded`, die Fixtures haben weder
//! Fillets noch Global-Round):
//!
//! 1. `l_extrude`   — L-Profil-Extrude, Erosion δ (Reflex-Ecke → Bogenwand).
//! 2. `pocket`      — Box minus Box, Erosion δ (−δ durch Difference,
//!                    Subtrahend dilatiert per f−δ am Blatt).
//! 3. `wedge_int`   — Dieder-Sweep: zwei Boxen im Winkel α per
//!                    INTERSECTION, Erosion δ (exakter Pushdown-Fall;
//!                    misst den Metrikfehler der max-Naht über α).
//! 4. `notch_union` — Dieder-Sweep dual: dieselben Boxen per UNION
//!                    (konkave Materialnaht, Luftkeil-Öffnung α),
//!                    Erosion δ — der DUALE STOPP-FALL (−δ an Union
//!                    bleibt f−δ). Vorhersage: Nullmengenfehler
//!                    (fehlendes Material) der Tiefe δ·(1/sin(α/2) − 1).
//! 5. `shell`       — Box-Shell in beiden Bauarten: `expand` =
//!                    Difference(Box, Offset(Box, −t)) (so baut Phase D
//!                    Shell) und `field` = heutiger `CsgNode::Shell`-
//!                    Modifier max(d, −(d+t)).
//!
//! Ground Truth: Brute-Force-Distanz (metricity-lab-Harness) zur
//! Nullmenge eines **analytischen, vorzeichen-exakten Referenzfeldes
//! des WAHR erodierten Solids** (Erosions-Mengenidentitäten:
//! (A∩B)↓δ = A↓δ ∩ B↓δ, (A∖B)↓δ = A↓δ ∖ B↑δ, Extrude↓δ =
//! Extrude(Profil↓δ, h−δ); für die Union der Kegel-Abstand
//! δ − dist(p, Luftkeil)). Metriken getrennt nach Naht-Band
//! (≤ 1 mm von der wahren Naht-Kurve), Naht-Region (≤ δ+1 mm) und
//! Fernfeld; Nullmengen-Abstand über die Oberflächen-Punktwolke des
//! IST-Feldes gegen die wahre GT; Vorzeichen-Fehlklassifikation;
//! ‖∇f‖-Statistik im Band (Normfehler- vs. Fußpunktfehler-Taxonomie,
//! SYNTHESIS §1).
//!
//! Aufruf: cargo run -p metricity-lab --release --bin b0_concave_seam

use std::path::PathBuf;

use fielddraft_kernel::csg::CsgNode;
use fielddraft_kernel::sketch2d::{Sketch2d, Sketch2dPrimitive};
use fielddraft_protocol::{FeatureId, Primitive, Transform};

use metricity_lab::field::{CsgField, Field};
use metricity_lab::ground_truth::{central_gradient, GroundTruth, GtOptions};
use metricity_lab::metrics::{sample_near_surface, sample_uniform, Stats};
use metricity_lab::report::write_csv;
use metricity_lab::util::{add, scale, SplitMix64};
use rayon::prelude::*;

fn out_dir() -> PathBuf {
    PathBuf::from("docs/research/sdf-metricity/iter3-offset-tree")
}

// ── Analytische f64-Bausteine der Referenzfelder ───────────────────

/// Exaktes Box-SDF (f64).
fn sbox(p: [f64; 3], c: [f64; 3], h: [f64; 3]) -> f64 {
    let q = [
        (p[0] - c[0]).abs() - h[0],
        (p[1] - c[1]).abs() - h[1],
        (p[2] - c[2]).abs() - h[2],
    ];
    let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2) + q[2].max(0.0).powi(2)).sqrt();
    let inside = q[0].max(q[1]).max(q[2]).min(0.0);
    outside + inside
}

/// Exaktes Polygon-SDF in 2D (iq, negatives Inneres, CCW/CW-egal).
fn poly_sdf2(p: [f64; 2], v: &[[f64; 2]]) -> f64 {
    let n = v.len();
    let mut d = f64::INFINITY;
    let mut s = 1.0f64;
    let mut j = n - 1;
    for i in 0..n {
        let e = [v[j][0] - v[i][0], v[j][1] - v[i][1]];
        let w = [p[0] - v[i][0], p[1] - v[i][1]];
        let t = ((w[0] * e[0] + w[1] * e[1]) / (e[0] * e[0] + e[1] * e[1])).clamp(0.0, 1.0);
        let b = [w[0] - e[0] * t, w[1] - e[1] * t];
        d = d.min(b[0] * b[0] + b[1] * b[1]);
        let c0 = p[1] >= v[i][1];
        let c1 = p[1] < v[j][1];
        let c2 = e[0] * w[1] > e[1] * w[0];
        if (c0 && c1 && c2) || (!c0 && !c1 && !c2) {
            s = -s;
        }
        j = i;
    }
    s * d.sqrt()
}

fn hypot2(a: f64, b: f64) -> f64 {
    (a * a + b * b).sqrt()
}

/// Abstand eines 2D-Punkts zum Strahl {t·r, t ≥ 0} (r normiert).
fn ray_dist2(p: [f64; 2], r: [f64; 2]) -> f64 {
    let t = (p[0] * r[0] + p[1] * r[1]).max(0.0);
    hypot2(p[0] - t * r[0], p[1] - t * r[1])
}

/// Referenzfeld / Naht-Abstand als boxed Closures (per Case).
type FieldFn = Box<dyn Fn([f64; 3]) -> f64 + Sync + Send>;

struct AnalyticField<'a>(&'a (dyn Fn([f64; 3]) -> f64 + Sync + Send));

impl Field for AnalyticField<'_> {
    fn eval(&self, p: [f64; 3]) -> f64 {
        (self.0)(p)
    }
}

/// Erwartete Baumform nach der Produktions-Normalisierung.
#[derive(PartialEq, Clone, Copy, Debug)]
enum PipeShape {
    /// Kein Offset-Knoten übrig (alles gefaltet).
    Clean,
    /// Dilations-`Offset(Leaf, +δ)` bleibt stehen (exakt per f−δ).
    DilationLeaf,
    /// Wurzel bleibt `Offset(Boolean, −δ)` — der duale Stopp-Fall.
    StopRoot,
}

fn tree_has_offset(n: &CsgNode) -> bool {
    match n {
        CsgNode::Offset(_, _) => true,
        CsgNode::Union(a, b)
        | CsgNode::Difference(a, b)
        | CsgNode::Intersection(a, b) => tree_has_offset(a) || tree_has_offset(b),
        CsgNode::Transform(_, inner) | CsgNode::Shell(inner, _, _) => tree_has_offset(inner),
        _ => false,
    }
}

struct Case {
    fixture: &'static str,
    variant: &'static str,
    alpha_deg: f64,
    delta: f64,
    grid: usize,
    lo: [f64; 3],
    hi: [f64; 3],
    node: CsgNode,
    shape: PipeShape,
    reference: FieldFn,
    seam: FieldFn,
    /// Analytische Vorhersage: notch_union = Sliver-Tiefe
    /// δ·(1/sin(α/2) − 1) [mm]; wedge_int = Metrik-Faktor
    /// (1 − sin(α/2)) [mm Fehler je mm Naht-Abstand]; sonst 0.
    pred_mm: f64,
}

// ── Fixtures ───────────────────────────────────────────────────────

fn leaf_box(id: u32, c: [f32; 3], h: [f32; 3]) -> CsgNode {
    CsgNode::Leaf {
        id: FeatureId::from_u32(id),
        primitive: Primitive::Box { center: c, half_extents: h },
    }
}

/// 1) L-Profil-Extrude, Erosion δ. Reflex-Ecke bei (12,12); die wahre
/// Erosion rundet sie zu einem Bogen r=δ (Innen-Offset), die Deckel
/// rücken auf ±(h−δ).
fn l_extrude_case(delta: f64) -> Case {
    const H: f64 = 12.0;
    let verts: [[f64; 2]; 6] =
        [[0.0, 0.0], [40.0, 0.0], [40.0, 12.0], [12.0, 12.0], [12.0, 40.0], [0.0, 40.0]];
    let node = CsgNode::Offset(
        Box::new(CsgNode::Extrude {
            id: FeatureId::from_u32(1),
            sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Polygon {
                vertices: verts.iter().map(|v| [v[0] as f32, v[1] as f32]).collect(),
            })),
            half_height: H as f32,
            wall_subs: None,
        }),
        -(delta as f32),
    );
    let d = delta;
    let reference: FieldFn = Box::new(move |p: [f64; 3]| {
        (poly_sdf2([p[0], p[1]], &verts) + d).max(p[2].abs() - (H - d))
    });
    // Naht = Bogenwand um (12,12) (Sektor 180°–270°), vertikal über
    // |z| ≤ H−δ; darüber hinaus zählt der Deckelrand-Bogen via dz.
    let seam: FieldFn = Box::new(move |p: [f64; 3]| {
        let v = [p[0] - 12.0, p[1] - 12.0];
        let d2 = if v[0] <= 0.0 && v[1] <= 0.0 {
            (hypot2(v[0], v[1]) - d).abs()
        } else {
            hypot2(p[0] - (12.0 - d), p[1] - 12.0)
                .min(hypot2(p[0] - 12.0, p[1] - (12.0 - d)))
        };
        let dz = (p[2].abs() - (H - d)).max(0.0);
        hypot2(d2, dz)
    });
    Case {
        fixture: "l_extrude",
        variant: "pipeline",
        alpha_deg: 0.0,
        delta,
        grid: 96,
        lo: [0.0, 0.0, -H],
        hi: [40.0, 40.0, H],
        node,
        shape: PipeShape::Clean,
        reference,
        seam,
        pred_mm: 0.0,
    }
}

/// 2) Box minus Box (Tasche), Erosion δ. Wahr: A↓δ ∖ B↑δ. Die scharfe
/// konkave Naht ist der Taschenmund-Rand (Rounded-Rect bei z = 15−δ);
/// der Boden-Wand-Übergang ist durch die Dilation von B tangential.
fn pocket_case(delta: f64) -> Case {
    let a_h = [30.0, 20.0, 15.0];
    let b_c = [0.0, 0.0, 15.0];
    let b_h = [12.0, 7.0, 12.0];
    let node = CsgNode::Offset(
        Box::new(CsgNode::Difference(
            Box::new(leaf_box(1, [0.0; 3], [30.0, 20.0, 15.0])),
            Box::new(leaf_box(2, [0.0, 0.0, 15.0], [12.0, 7.0, 12.0])),
        )),
        -(delta as f32),
    );
    let d = delta;
    let reference: FieldFn = Box::new(move |p: [f64; 3]| {
        let a = sbox(p, [0.0; 3], [a_h[0] - d, a_h[1] - d, a_h[2] - d]);
        let b = sbox(p, b_c, b_h) - d;
        a.max(-b)
    });
    let seam: FieldFn = Box::new(move |p: [f64; 3]| {
        // Mundrand: Rounded-Rect-Kontur (Halbmaße 12×7, Radius δ) bei
        // z = 15 − δ.
        let q = [p[0].abs() - b_h[0], p[1].abs() - b_h[1]];
        let rr = hypot2(q[0].max(0.0), q[1].max(0.0)) + q[0].max(q[1]).min(0.0) - d;
        hypot2(rr.abs(), p[2] - (a_h[2] - d))
    });
    Case {
        fixture: "pocket",
        variant: "pipeline",
        alpha_deg: 0.0,
        delta,
        grid: 96,
        lo: [-30.0, -20.0, -15.0],
        hi: [30.0, 20.0, 15.0],
        node,
        shape: PipeShape::DilationLeaf,
        reference,
        seam,
        pred_mm: 0.0,
    }
}

/// 3)+4) Dieder: zwei große Boxen, deren Deckflächen sich in der
/// z-Achse schneiden. `union_=false`: Intersection → Materialkeil der
/// Öffnung α (exakter Pushdown, Blätter gefaltet). `union_=true`:
/// Union → konkave Naht mit Luftkeil-Öffnung α (dualer Stopp-Fall,
/// Wurzel bleibt f−δ). Messfenster ±25/±20 mm um die Naht; die Boxen
/// sind so groß, dass im Fenster nur die beiden Deckflächen leben.
fn dihedral_case(alpha_deg: f64, delta: f64, union_: bool) -> Case {
    let alpha = alpha_deg.to_radians();
    let gamma = std::f64::consts::PI - alpha;
    let n2 = [-gamma.sin(), gamma.cos()];
    let b1 = CsgNode::Transform(
        Transform {
            translate: [0.0, -50.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(1, [0.0; 3], [100.0, 50.0, 50.0])),
    );
    let b2 = CsgNode::Transform(
        Transform {
            translate: [
                (50.0 * gamma.sin()) as f32,
                (-50.0 * gamma.cos()) as f32,
                0.0,
            ],
            rotation: [0.0, 0.0, (gamma / 2.0).sin() as f32, (gamma / 2.0).cos() as f32],
            scale: [1.0; 3],
        },
        Box::new(leaf_box(2, [0.0; 3], [100.0, 50.0, 50.0])),
    );
    let inner = if union_ {
        CsgNode::Union(Box::new(b1), Box::new(b2))
    } else {
        CsgNode::Intersection(Box::new(b1), Box::new(b2))
    };
    let node = CsgNode::Offset(Box::new(inner), -(delta as f32));
    let d = delta;

    let (reference, seam, pred): (FieldFn, FieldFn, f64) = if union_ {
        // Luftkeil = {p·n1 ≥ 0 ∧ p·n2 ≥ 0} (2D-Kegel, z-invariant);
        // wahre Erosion: Material ⇔ dist(p, Keil) ≥ δ.
        let r1 = [-1.0f64, 0.0]; // ⊥ n1, in den Keil-Rand
        let r2 = [gamma.cos(), gamma.sin()]; // ⊥ n2
        let reference: FieldFn = Box::new(move |p: [f64; 3]| {
            let q = [p[0], p[1]];
            let inside_cone = q[1] >= 0.0 && (q[0] * n2[0] + q[1] * n2[1]) >= 0.0;
            let dist = if inside_cone {
                0.0
            } else {
                ray_dist2(q, r1).min(ray_dist2(q, r2))
            };
            d - dist
        });
        // Wahre Naht-Kurve = Bogen r=δ um die Scheitelachse (Sektor,
        // dessen nächster Keil-Punkt der Scheitel ist).
        let seam: FieldFn = Box::new(move |p: [f64; 3]| {
            let q = [p[0], p[1]];
            let behind = (q[0] * r1[0] + q[1] * r1[1]) <= 0.0
                && (q[0] * r2[0] + q[1] * r2[1]) <= 0.0;
            if behind {
                (hypot2(q[0], q[1]) - d).abs()
            } else {
                // Bogen-Endpunkte: −δ·n1 und −δ·n2.
                hypot2(q[0], q[1] + d).min(hypot2(q[0] + d * n2[0], q[1] + d * n2[1]))
            }
        });
        (reference, seam, d * (1.0 / (alpha / 2.0).sin() - 1.0))
    } else {
        let reference: FieldFn = Box::new(move |p: [f64; 3]| {
            (p[1] + d).max(p[0] * n2[0] + p[1] * n2[1] + d)
        });
        // Erodierte Keilkante: n1·e = −δ ∧ n2·e = −δ.
        let e = [d * (gamma / 2.0).tan(), -d];
        let seam: FieldFn =
            Box::new(move |p: [f64; 3]| hypot2(p[0] - e[0], p[1] - e[1]));
        (reference, seam, 1.0 - (alpha / 2.0).sin())
    };

    Case {
        fixture: if union_ { "notch_union" } else { "wedge_int" },
        variant: "pipeline",
        alpha_deg,
        delta,
        grid: 96,
        lo: [-25.0, -25.0, -20.0],
        hi: [25.0, 25.0, 20.0],
        node,
        shape: if union_ { PipeShape::StopRoot } else { PipeShape::Clean },
        reference,
        seam,
        pred_mm: pred,
    }
}

/// 5) Box-Shell, Wandstärke t. `expand` = Difference(Box, Offset(Box,−t))
/// (Phase-D-Expansion, Fold → geschrumpfte Box); `field` = heutiger
/// `CsgNode::Shell`-Modifier max(d, −(d+t)). Wahre Referenz für beide:
/// max(box, −box↓t). Naht = die 12 Innenkanten der Kavität.
fn shell_case(t: f64, expand: bool) -> Case {
    let h = [25.0, 18.0, 12.0];
    let node = if expand {
        CsgNode::Difference(
            Box::new(leaf_box(1, [0.0; 3], [25.0, 18.0, 12.0])),
            Box::new(CsgNode::Offset(
                Box::new(leaf_box(2, [0.0; 3], [25.0, 18.0, 12.0])),
                -(t as f32),
            )),
        )
    } else {
        CsgNode::Shell(
            Box::new(leaf_box(1, [0.0; 3], [25.0, 18.0, 12.0])),
            t as f32,
            Vec::new(),
        )
    };
    let reference: FieldFn = Box::new(move |p: [f64; 3]| {
        let outer = sbox(p, [0.0; 3], h);
        let inner = sbox(p, [0.0; 3], [h[0] - t, h[1] - t, h[2] - t]);
        outer.max(-inner)
    });
    let (a, b, c) = (h[0] - t, h[1] - t, h[2] - t);
    let seam: FieldFn = Box::new(move |p: [f64; 3]| {
        let (x, y, z) = (p[0].abs(), p[1].abs(), p[2].abs());
        let ez = ((x - a).powi(2) + (y - b).powi(2) + (z - c).max(0.0).powi(2)).sqrt();
        let ex = ((y - b).powi(2) + (z - c).powi(2) + (x - a).max(0.0).powi(2)).sqrt();
        let ey = ((x - a).powi(2) + (z - c).powi(2) + (y - b).max(0.0).powi(2)).sqrt();
        ez.min(ex).min(ey)
    });
    Case {
        fixture: "shell",
        variant: if expand { "expand" } else { "field" },
        alpha_deg: 0.0,
        delta: t,
        grid: if t < 1.0 { 200 } else { 96 },
        lo: [-25.0, -18.0, -12.0],
        hi: [25.0, 18.0, 12.0],
        node,
        shape: PipeShape::Clean,
        reference,
        seam,
        pred_mm: 0.0,
    }
}

fn cases() -> Vec<Case> {
    let mut v = Vec::new();
    for d in [0.5, 2.0, 5.0] {
        v.push(l_extrude_case(d));
    }
    for d in [0.5, 2.0, 5.0] {
        v.push(pocket_case(d));
    }
    for union_ in [false, true] {
        for a in [60.0, 90.0, 120.0, 150.0, 170.0] {
            v.push(dihedral_case(a, 2.0, union_));
        }
        v.push(dihedral_case(90.0, 0.5, union_));
        v.push(dihedral_case(90.0, 5.0, union_));
    }
    for t in [0.5, 2.0, 5.0] {
        v.push(shell_case(t, true));
        v.push(shell_case(t, false));
    }
    v
}

// ── Messlauf ───────────────────────────────────────────────────────

const BAND_R: f64 = 1.0; // Naht-Band [mm]
const SEED: u64 = 4242;

fn stats_cells(s: &Stats) -> String {
    format!("{},{:.6},{:.6},{:.6},{:.6}", s.count, s.mean, s.p95, s.max, s.mean_signed)
}

pub const HEADER: &str = "fixture,variant,alpha_deg,delta_mm,grid,model_size,n_gt_surface,\
band_n,band_mean,band_p95,band_max,band_bias,\
region_n,region_mean,region_p95,region_max,region_bias,\
far_n,far_mean,far_p95,far_max,far_bias,\
zs_band_n,zs_band_mean,zs_band_max,zs_region_max,zs_all_max,\
mis_n,mis_max_depth_mm,mis_far_n,mis_near_n,mis_near_total,\
grad_band_n,grad_band_mean_dev,grad_band_max_dev,\
pred_mm";

fn run_case(case: &Case) -> String {
    let t0 = std::time::Instant::now();
    let pipe = case.node.clone().push_down_offsets().fold_offset_leaves();
    match case.shape {
        PipeShape::Clean => assert!(
            !tree_has_offset(&pipe),
            "{}/{} δ={}: erwartet vollständig gefaltet",
            case.fixture,
            case.variant,
            case.delta
        ),
        PipeShape::DilationLeaf => {
            assert!(
                !matches!(pipe, CsgNode::Offset(_, _)) && tree_has_offset(&pipe),
                "{}: erwartet Dilations-Offset am Blatt, Wurzel boolesch",
                case.fixture
            );
        }
        PipeShape::StopRoot => assert!(
            matches!(pipe, CsgNode::Offset(_, _)),
            "{}: erwartet Stopp (Wurzel-Offset, f−δ)",
            case.fixture
        ),
    }
    let ist = CsgField::visual(&pipe);
    let reference = AnalyticField(&*case.reference);
    let opts = GtOptions { grid: case.grid, refine: true, ..GtOptions::default() };
    let gt = GroundTruth::build(&reference, case.lo, case.hi, &opts);
    let model = gt.model_size;
    let region_r = case.delta + 1.0;
    let off = (case.delta + 1.5).max(3.0);

    // Punktmengen: uniform + oberflächennah + Band-/Region-gezielt
    // (Rejection über den Naht-Abstand aus der GT-Punktwolke).
    let mut pts = sample_uniform(gt.lo, gt.hi, 2000, SEED);
    pts.extend(sample_near_surface(&gt, 3000, off / model, SEED ^ 0xA5A5));
    {
        let cloud = gt.surface_points();
        let mut rng = SplitMix64::new(SEED ^ 0xB0B0_5EA4);
        let (mut n_band, mut n_region, mut tries) = (0usize, 0usize, 0usize);
        while (n_band < 2000 || n_region < 2000) && tries < 600_000 {
            tries += 1;
            let q = cloud[(rng.next_u64() as usize) % cloud.len()];
            let p = add(q, scale(rng.unit_dir(), rng.range(-off, off)));
            let s = (case.seam)(p);
            if s <= BAND_R && n_band < 2000 {
                pts.push(p);
                n_band += 1;
            } else if s <= region_r && n_region < 2000 {
                pts.push(p);
                n_region += 1;
            }
        }
    }

    // (p, d_wahr, f_ist, naht_abstand)
    let rows: Vec<([f64; 3], f64, f64, f64)> = pts
        .par_iter()
        .map(|&p| (p, gt.distance(p), ist.eval(p), (case.seam)(p)))
        .filter(|(_, d, f, s)| d.is_finite() && f.is_finite() && s.is_finite())
        .collect();

    let split = |pred: &dyn Fn(f64) -> bool| -> Stats {
        Stats::from_signed(
            rows.iter().filter(|(_, _, _, s)| pred(*s)).map(|(_, d, f, _)| f - d).collect(),
        )
    };
    let band = split(&|s| s <= BAND_R);
    let region = split(&|s| s <= region_r);
    let far = split(&|s| s > region_r);

    // Vorzeichen-Fehlklassifikation (Feld sagt außen, wahr innen o. u.).
    let mis: Vec<(f64, f64)> = rows
        .iter()
        .filter(|(_, d, f, _)| (*d < 0.0) != (*f < 0.0))
        .map(|(_, d, _, s)| (d.abs(), *s))
        .collect();
    let mis_max_depth = mis.iter().map(|(a, _)| *a).fold(0.0f64, f64::max);
    let mis_far_n = mis.iter().filter(|(_, s)| *s > region_r).count();
    let mis_near_n = rows
        .iter()
        .filter(|(_, d, f, _)| d.abs() <= 1.0 && (*d < 0.0) != (*f < 0.0))
        .count();
    let mis_near_total = rows.iter().filter(|(_, d, _, _)| d.abs() <= 1.0).count();

    // Nullmengen-Abstand: IST-Oberflächenwolke gegen die wahre GT.
    let gt_ist_opts = GtOptions { grid: case.grid, refine: false, ..GtOptions::default() };
    let gt_ist = GroundTruth::build(&ist, case.lo, case.hi, &gt_ist_opts);
    let sp = gt_ist.surface_points();
    let stride = (sp.len() / 2400).max(1);
    let zs: Vec<(f64, f64)> = sp
        .par_iter()
        .step_by(stride)
        .map(|&q| ((case.seam)(q), gt.distance(q).abs()))
        .collect();
    let zs_band: Vec<f64> = zs.iter().filter(|(s, _)| *s <= BAND_R).map(|(_, e)| *e).collect();
    let zs_band_stats = Stats::from_signed(zs_band);
    let zs_region_max = zs
        .iter()
        .filter(|(s, _)| *s <= region_r)
        .map(|(_, e)| *e)
        .fold(0.0f64, f64::max);
    let zs_all_max = zs.iter().map(|(_, e)| *e).fold(0.0f64, f64::max);

    // ‖∇f‖ im Band (Normfehler- vs. Fußpunktfehler-Klassifikation).
    let h = model * 1e-3;
    let grad_devs: Vec<f64> = rows
        .par_iter()
        .filter(|(_, _, _, s)| *s <= BAND_R)
        .map(|(p, _, _, _)| {
            let g = central_gradient(&ist, *p, h);
            (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt() - 1.0
        })
        .filter(|d| d.is_finite())
        .collect();
    let grad = Stats::from_signed(grad_devs);

    println!(
        "  band max|f−d| {:.4}  zs_band max {:.4}  zs_all max {:.4}  mis {} (tiefste {:.4})  pred {:.4}  [{:.1}s]",
        band.max,
        zs_band_stats.max,
        zs_all_max,
        mis.len(),
        mis_max_depth,
        case.pred_mm,
        t0.elapsed().as_secs_f64()
    );
    if mis_far_n > 0 {
        println!("  WARNUNG: {mis_far_n} Vorzeichen-Fehler AUSSERHALB der Naht-Region (Konstruktion prüfen!)");
    }

    [
        format!(
            "{},{},{},{},{},{:.3},{}",
            case.fixture,
            case.variant,
            case.alpha_deg,
            case.delta,
            case.grid,
            model,
            gt.n_surface_points()
        ),
        stats_cells(&band),
        stats_cells(&region),
        stats_cells(&far),
        format!(
            "{},{:.6},{:.6},{:.6},{:.6}",
            zs_band_stats.count, zs_band_stats.mean, zs_band_stats.max, zs_region_max, zs_all_max
        ),
        format!("{},{:.6},{},{},{}", mis.len(), mis_max_depth, mis_far_n, mis_near_n, mis_near_total),
        format!("{},{:.6},{:.6}", grad.count, grad.mean, grad.max),
        format!("{:.6}", case.pred_mm),
    ]
    .join(",")
}

fn main() {
    std::fs::create_dir_all(out_dir()).expect("mkdir out");
    let all = cases();
    println!("── B0: konkave Shell-Naht — {} Fälle ──", all.len());
    let mut rows = Vec::new();
    for case in &all {
        println!(
            "{}/{} α={}° δ={} (grid {})",
            case.fixture, case.variant, case.alpha_deg, case.delta, case.grid
        );
        rows.push(run_case(case));
    }
    let path = out_dir().join("b0_concave_seam.csv");
    write_csv(&path, HEADER, &rows).expect("write csv");
    println!("CSV → {}", path.display());
}
