//! #384 Stufe 2 — Verifikation des Compile-Zeit-Lipschitz-Netzes.
//!
//! Vergleicht je Szene die ALTE `march_relax`-Politik (handgeschätzte
//! Pauschalen: Revolve 0.7, Twist/Bend/Gyroid/Chamfer 1.0) mit der NEUEN
//! hergeleiteten `1/L`-Schranke (`sdf_compiler::lipschitz_bound`):
//! Sphere-Trace-Penetrationsrate (Tunnel) und Schrittzahl, gemessen gegen
//! die Ground Truth der Tape-EIGENEN Nullmenge (kein Feld-Mismatch).
//!
//! Zusätzlich: Überschätzungs-Check des Revolve-Felds (max f/d), der die
//! Ablösung der 0.7-Pauschale empirisch deckt.
//!
//! Lauf: `cargo run -p metricity-lab --release --bin stage2_relax_net`

use fielddraft_kernel::bvh::BoundedCsg;
use fielddraft_kernel::csg::{BlendMode, CsgNode};
use fielddraft_kernel::sdf_compiler::{compile_body, eval_opcodes_with_volumes, CompiledSdf};
use fielddraft_kernel::sketch2d::{Sketch2d, Sketch2dPrimitive};
use fielddraft_protocol::{FeatureId, Primitive, Transform};

use metricity_lab::scenes;
use metricity_lab::util::{add, normalize, scale, sub, SplitMix64};
use metricity_lab::{Field, GroundTruth, GtOptions, TapeField};

fn tape_eval(c: &CompiledSdf, p: [f64; 3]) -> f64 {
    eval_opcodes_with_volumes(&c.opcodes, [p[0] as f32, p[1] as f32, p[2] as f32], &c.volumes)
        as f64
}

struct TraceStats {
    hits: usize,
    misses: usize,
    pen: usize,
    pen_max: f64,
    steps_mean: f64,
}

/// Sphere-Trace von außen auf GT-Oberflächenpunkte, Schritt
/// `t += max(f·relax, ε)`; Penetration = Hit hinter der wahren Fläche.
fn trace(
    compiled: &CompiledSdf,
    gt: &GroundTruth<'_>,
    model: f64,
    relax: f64,
    n_rays: usize,
    seed: u64,
) -> TraceStats {
    let mut rng = SplitMix64::new(seed);
    let surf = gt.surface_points();
    let tol_hit = 2e-4 * model;
    let pen_tol = 1e-3 * model;
    let eps = tol_hit * 0.5;
    let t_max = model * 2.5;
    let launch = 0.35 * model;
    let max_steps = 8000usize;

    let (mut hits, mut misses, mut pen) = (0usize, 0usize, 0usize);
    let mut pen_max = 0.0f64;
    let mut steps_sum = 0usize;

    for _ in 0..n_rays {
        let q = surf[(rng.next_u64() as usize) % surf.len()];
        let mut o = [0.0f64; 3];
        let mut ok = false;
        for _ in 0..16 {
            let u = rng.unit_dir();
            o = add(q, scale(u, launch));
            if tape_eval(compiled, o) > 0.05 * model {
                ok = true;
                break;
            }
        }
        if !ok {
            continue;
        }
        let jit = scale(rng.unit_dir(), 0.02 * model);
        let dir = match normalize(sub(add(q, jit), o)) {
            Some(d) => d,
            None => continue,
        };
        let (mut t, mut steps, mut hit) = (0.0f64, 0usize, false);
        while steps < max_steps && t < t_max {
            let p = add(o, scale(dir, t));
            let f = tape_eval(compiled, p);
            if f < tol_hit {
                hit = true;
                hits += 1;
                steps_sum += steps;
                let d_true = gt.distance(p);
                if d_true < -pen_tol {
                    pen += 1;
                    pen_max = pen_max.max(-d_true);
                }
                break;
            }
            t += (f * relax).max(eps);
            steps += 1;
        }
        if !hit {
            misses += 1;
        }
    }
    TraceStats {
        hits,
        misses,
        pen,
        pen_max,
        steps_mean: if hits > 0 { steps_sum as f64 / hits as f64 } else { 0.0 },
    }
}

/// Revolve-Ring (Rechteck-Profil r ∈ [3, 7], y ∈ [−4, 4]) — die
/// qmvn-Szenenfamilie, für die die 0.7-Pauschale eingeführt wurde.
fn revolve_ring() -> (String, CsgNode) {
    let node = CsgNode::Revolve {
        wall_subs: None,
        id: FeatureId::from_u32(1),
        sketch: Box::new(Sketch2d::Prim(Sketch2dPrimitive::Rectangle {
            center: [5.0, 0.0],
            half_extents: [2.0, 4.0],
        })),
    };
    ("revolve_ring".into(), node)
}

/// Chamfer-Union zweier Boxen, eine um 30° um Z gedreht — spitzwinklige
/// Wände, an denen die Schnittebene (a+b−r)/√2 die Distanz real
/// überschätzt (Wachstumsrate 2·cos(θ/2)/√2 > 1).
fn chamfer_plates() -> (String, CsgNode) {
    let plate = |id: u32, rot: [f32; 4]| {
        CsgNode::Transform(
            Transform { translate: [0.0; 3], rotation: rot, scale: [1.0; 3] },
            Box::new(CsgNode::Leaf {
                id: FeatureId::from_u32(id),
                primitive: Primitive::Box { center: [0.0; 3], half_extents: [30.0, 4.0, 20.0] },
            }),
        )
    };
    let a15 = 15.0f32.to_radians();
    let node = CsgNode::SmoothUnion(
        Box::new(plate(1, [0.0, 0.0, 0.0, 1.0])),
        Box::new(plate(2, [0.0, 0.0, a15.sin(), a15.cos()])),
        3.0,
        BlendMode::Chamfer,
    );
    ("chamfer_plates".into(), node)
}

fn main() {
    // (Name, Node, alte Pauschal-Politik vor #384 Stufe 2)
    let mut cases: Vec<(String, CsgNode, f64)> = Vec::new();
    for s in [scenes::twist_box(), scenes::bend_bar(), scenes::gyroid_box(), scenes::twist_thin_wall()] {
        cases.push((s.name.clone(), s.node, 1.0)); // Twist/Bend/Gyroid: alt ungedämpft
    }
    let (n, node) = revolve_ring();
    cases.push((n, node, 0.7)); // alt: REVOLVE_MARCH_RELAX
    let (n, node) = chamfer_plates();
    cases.push((n, node, 1.0)); // alt: Chamfer ungedämpft

    println!(
        "{:<16} {:>9} {:>9} | {:>8} {:>8} {:>7} {:>6} | {:>8} {:>8} {:>7} {:>6}",
        "szene", "relax_alt", "relax_neu", "pen_alt", "penmax", "steps", "miss", "pen_neu",
        "penmax", "steps", "miss"
    );

    for (name, node, old_relax) in &cases {
        let bounded = BoundedCsg::from_csg(node);
        let compiled = compile_body(&bounded);
        let new_relax = compiled.march_relax as f64;

        let aabb = node.aabb();
        let lo = [aabb.min[0] as f64, aabb.min[1] as f64, aabb.min[2] as f64];
        let hi = [aabb.max[0] as f64, aabb.max[1] as f64, aabb.max[2] as f64];
        let model = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]);

        let tape = TapeField { compiled: &compiled };
        let gt = GroundTruth::build(&tape, lo, hi, &GtOptions::default());

        let n_rays = 400;
        let a = trace(&compiled, &gt, model, *old_relax, n_rays, 0xD00D);
        let b = trace(&compiled, &gt, model, new_relax, n_rays, 0xD00D);

        println!(
            "{:<16} {:>9.3} {:>9.3} | {:>7.1}% {:>8.3} {:>7.1} {:>6} | {:>7.1}% {:>8.3} {:>7.1} {:>6}",
            name,
            old_relax,
            new_relax,
            100.0 * a.pen as f64 / a.hits.max(1) as f64,
            a.pen_max,
            a.steps_mean,
            a.misses,
            100.0 * b.pen as f64 / b.hits.max(1) as f64,
            b.pen_max,
            b.steps_mean,
            b.misses,
        );

        // Überschätzungs-Check: max f/d über oberflächennahe + uniforme Punkte
        // (deckt für revolve_ring die Ablösung der 0.7-Pauschale).
        if name == "revolve_ring" {
            let mut rng = SplitMix64::new(0xBEEF);
            let mut max_ratio = 0.0f64;
            let margin = 0.2 * model;
            for _ in 0..20000 {
                let p = [
                    rng.range(lo[0] - margin, hi[0] + margin),
                    rng.range(lo[1] - margin, hi[1] + margin),
                    rng.range(lo[2] - margin, hi[2] + margin),
                ];
                let f = tape.eval(p).abs();
                let d = gt.distance(p).abs();
                if d > 1e-3 * model {
                    max_ratio = max_ratio.max(f / d);
                }
            }
            println!("    revolve_ring: max |f|/|d| = {max_ratio:.4} (1-Lipschitz ⇔ ≤ ~1)");
        }
    }
}
