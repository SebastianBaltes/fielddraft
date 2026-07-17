//! Adversarialer Testkorpus: benannte, reproduzierbare Szenen direkt
//! gegen die Kernel-API (`CsgNode`) gebaut. Modellgröße ~100 mm.
//!
//! Alle Konstruktoren sind deterministisch (Platzierung tiefer Bäume
//! via SplitMix64 mit festem Seed). `scene_by_name` versteht zusätzlich
//! die parametrischen Familien `deep_csg_<N>` und `smin_chain_<N>`.
//!
//! Zu Twist/Bend/Gyroid: alle drei sind direkt in der Kernel-API
//! erreichbar (`CsgNode::Twist`/`Bend`, `Primitive::Gyroid`) und daher
//! im Korpus. Der Gyroid-AABB ist unendlich — die Szene hält deshalb
//! die Box als LINKEN Operanden der Intersection (deren AABB zählt).

use fielddraft_kernel::csg::{BlendMode, BlendProvenance, CsgNode};
use fielddraft_protocol::{FeatureId, Primitive, Transform};

use crate::util::SplitMix64;

pub struct Scene {
    pub name: String,
    pub description: String,
    pub node: CsgNode,
    /// Ground-Truth-Gitter-Override (0 = Default des Runners). Dünne
    /// Wände brauchen ein feineres Oberflächen-Sampling.
    pub gt_grid: usize,
}

fn leaf(i: u32, primitive: Primitive) -> CsgNode {
    CsgNode::Leaf { id: FeatureId::from_u32(i), primitive }
}

fn boxp(i: u32, center: [f32; 3], half: [f32; 3]) -> CsgNode {
    leaf(i, Primitive::Box { center, half_extents: half })
}

fn sphere(i: u32, center: [f32; 3], radius: f32) -> CsgNode {
    leaf(i, Primitive::Sphere { center, radius })
}

fn scene(name: &str, description: &str, node: CsgNode) -> Scene {
    Scene {
        name: name.to_string(),
        description: description.to_string(),
        node,
        gt_grid: 0,
    }
}

// ── Kalibrier-Szenen (exakte SDFs, GT-Selbstkontrolle) ─────────────

pub fn sphere_cal() -> Scene {
    scene("sphere", "Kalibrierung: exakte Kugel r=40", sphere(1, [0.0; 3], 40.0))
}

pub fn box_cal() -> Scene {
    scene(
        "box",
        "Kalibrierung: exakte Box 80×50×30",
        boxp(1, [0.0; 3], [40.0, 25.0, 15.0]),
    )
}

// ── Dünne Wand: Box minus Box (allseitige Wandstärke t) ────────────

/// Geschlossene Box-Schale: Außenbox Halbmaß 50, Innenbox 50 − t.
/// `t_rel` ist die Wandstärke relativ zur Modellgröße (100 mm);
/// 0.005 → 0.5 mm. Konkave Innenkanten + zwei fast parallele Flächen
/// im Abstand t — der klassische max(d, −d_in)-Stressfall.
pub fn thin_wall(t_rel: f64) -> Scene {
    let t = (t_rel * 100.0) as f32;
    let node = CsgNode::Difference(
        Box::new(boxp(1, [0.0; 3], [50.0, 50.0, 50.0])),
        Box::new(boxp(2, [0.0; 3], [50.0 - t, 50.0 - t, 50.0 - t])),
    );
    Scene {
        name: format!("thin_wall_{}", fmt_permille(t_rel)),
        description: format!(
            "Box minus Box, allseitige Wandstaerke {t} mm ({} % der Modellgroesse)",
            t_rel * 100.0
        ),
        node,
        // Das Linien-Sampling muss in die Wand treffen: Gitterabstand
        // deutlich unter t halten (bis hinunter zu t = 0.5 mm).
        gt_grid: if t_rel < 0.015 { 192 } else { 0 },
    }
}

// ── Fast tangentialer Boolescher Schnitt ───────────────────────────

/// Zwei Kugeln r=40, Union; `gap_rel` = Flächenabstand relativ zum
/// Radius (negativ = Überlappung → konkave Naht-Kreislinie).
pub fn tangent_spheres(gap_rel: f64) -> Scene {
    let r = 40.0f32;
    let gap = (gap_rel as f32) * r;
    let cx = r + gap * 0.5;
    let node = CsgNode::Union(
        Box::new(sphere(1, [-cx, 0.0, 0.0], r)),
        Box::new(sphere(2, [cx, 0.0, 0.0], r)),
    );
    Scene {
        name: format!("tangent_spheres_{}", fmt_signed_permille(gap_rel)),
        description: format!(
            "Union zweier Kugeln r=40, Flaechenabstand {:.2} mm (negativ = Ueberlappung)",
            gap
        ),
        node,
        gt_grid: 0,
    }
}

// ── Tiefer CSG-Baum ────────────────────────────────────────────────

/// N verschachtelte Booleans über einer Basisbox (Halbmaß 40):
/// abwechselnd Union (Kugel r=18) und Difference (Box Halbmaß 14),
/// deterministisch auf einer Kugelschale r≈40 um den Ursprung
/// platziert, sodass jede Operation die Oberfläche wirklich schneidet.
/// Tiefe = Anzahl Boolean-Knoten (bis 64).
pub fn deep_csg(n: usize) -> Scene {
    let mut rng = SplitMix64::new(0xD5EE_D0_0000 + 7);
    let mut node = boxp(0, [0.0; 3], [40.0, 40.0, 40.0]);
    for i in 0..n {
        let u = rng.unit_dir();
        let radius = 38.0 + 6.0 * rng.next_f64(); // Zentrum nahe der Oberfläche
        let c = [
            (u[0] * radius) as f32,
            (u[1] * radius) as f32,
            (u[2] * radius) as f32,
        ];
        let id = (i + 1) as u32;
        if i % 2 == 0 {
            node = CsgNode::Union(Box::new(node), Box::new(sphere(id, c, 18.0)));
        } else {
            node = CsgNode::Difference(Box::new(node), Box::new(boxp(id, c, [14.0, 14.0, 14.0])));
        }
    }
    Scene {
        name: format!("deep_csg_{n:02}"),
        description: format!(
            "Basisbox mit {n} verschachtelten Booleans (Union Kugel / Difference Box im Wechsel)"
        ),
        node,
        gt_grid: 0,
    }
}

// ── smin-Kette (SmoothUnion-Kaskade) ───────────────────────────────

/// Kette aus `n` Kugeln r=15 im Abstand 20 entlang x (leicht in y
/// gewellt), links-gefaltet mit `SmoothUnion` (Fillet, r_blend=6).
/// Jede Stufe addiert eine √2-Lipschitz-Blendzone.
pub fn smin_chain(n: usize) -> Scene {
    assert!(n >= 1);
    let x0 = -((n as f32 - 1.0) * 10.0);
    let mut node = sphere(1, [x0, 0.0, 0.0], 15.0);
    for i in 1..n {
        let x = x0 + i as f32 * 20.0;
        let y = 6.0 * (i as f32 * 0.9).sin();
        node = CsgNode::SmoothUnion(
            Box::new(node),
            Box::new(sphere((i + 1) as u32, [x, y, 0.0], 15.0)),
            6.0,
            BlendMode::Fillet,
        );
    }
    Scene {
        name: format!("smin_chain_{n:02}"),
        description: format!("Kaskade aus {n} Kugeln r=15, SmoothUnion r_blend=6 (zirkulaeres smin)"),
        node,
        gt_grid: 0,
    }
}

// ── Domain-Warps & Gyroid ──────────────────────────────────────────

pub fn twist_box() -> Scene {
    // 90° Verdrehung über 100 mm Höhe.
    let rate = std::f32::consts::FRAC_PI_2 / 100.0;
    scene(
        "twist_box",
        "Box 60x100x20, um Y getwistet (90 Grad ueber die Hoehe) — Metrik dehnt mit dem Radius",
        CsgNode::Twist(Box::new(boxp(1, [0.0; 3], [30.0, 50.0, 10.0])), rate),
    )
}

pub fn bend_bar() -> Scene {
    // 60° Bogen über 100 mm Länge.
    let rate = std::f32::consts::FRAC_PI_3 / 100.0;
    scene(
        "bend_bar",
        "Balken 100x16x16, in der XY-Ebene gebogen (60 Grad ueber die Laenge)",
        CsgNode::Bend(Box::new(boxp(1, [0.0; 3], [50.0, 8.0, 8.0])), rate),
    )
}

pub fn gyroid_box() -> Scene {
    scene(
        "gyroid_box",
        "Gyroid-Lattice (Zelle 18 mm) mit Box Halbmass 40 geclippt — nicht-1-Lipschitz-Leaf",
        CsgNode::Intersection(
            Box::new(boxp(1, [0.0; 3], [40.0, 40.0, 40.0])),
            Box::new(leaf(2, Primitive::Gyroid { cell_size: 18.0, thickness: 0.35 })),
        ),
    )
}

pub fn twist_thin_wall() -> Scene {
    // Kombination Warp × dünne Wand: getwistete 2-mm-Schale.
    let t = 2.0f32;
    let rate = std::f32::consts::FRAC_PI_2 / 100.0;
    let shell = CsgNode::Difference(
        Box::new(boxp(1, [0.0; 3], [30.0, 50.0, 30.0])),
        Box::new(boxp(2, [0.0; 3], [30.0 - t, 50.0 - t, 30.0 - t])),
    );
    scene(
        "twist_thin_wall",
        "Getwistete Box-Schale, Wandstaerke 2 mm — Domain-Warp auf duenner Wand",
        CsgNode::Twist(Box::new(shell), rate),
    )
}

// ── MaskedBlend (Kanten-Fillet-Pfad) ───────────────────────────────

/// Säule auf Grundplatte, Junction-Kanten per MaskedBlend gefillet
/// (r=6, Masken-Tube r=15 um den Kantenzug bei y=20). Deckt den
/// mode-unabhängigen Fillet-Pfad ab (unnormalisiertes zirkuläres smin
/// im `CsgNode`-Feld; das kompilierte Tape nutzt zusätzlich den
/// #382-Render-Fade und weicht messbar ab → separater Tape-Report).
pub fn masked_blend_edge() -> Scene {
    let base = boxp(1, [0.0, 10.0, 0.0], [40.0, 10.0, 40.0]);
    let column = boxp(2, [0.0, 45.0, 0.0], [12.0, 30.0, 12.0]);
    let ring = vec![
        [-12.0, 20.0, -12.0],
        [12.0, 20.0, -12.0],
        [12.0, 20.0, 12.0],
        [-12.0, 20.0, 12.0],
        [-12.0, 20.0, -12.0],
    ];
    scene(
        "masked_blend_edge",
        "Saeule auf Platte, Junction-Kanten via MaskedBlend gefillet (r=6, Maske r=15)",
        CsgNode::MaskedBlend {
            a: Box::new(base),
            b: Box::new(column),
            mask_polyline: ring,
            mask_radius: 15.0,
            blend_radius: 6.0,
            blend_mode: BlendMode::Fillet,
            is_difference: false,
            provenance: BlendProvenance::NONE,
        },
    )
}

// ── Rotierte Box (GT-Validierung nutzt sie ebenfalls) ──────────────

pub fn rotated_box() -> Scene {
    scene(
        "rot_box",
        "Box 80x50x30, rotiert (0.7 rad um normierte Achse [1,1,0.3])",
        rotated_box_node(),
    )
}

pub fn rotated_box_node() -> CsgNode {
    let axis = [1.0f32, 1.0, 0.3];
    let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let angle = 0.7f32;
    let (s, c) = (angle * 0.5).sin_cos();
    let t = Transform {
        translate: [0.0; 3],
        rotation: [axis[0] / len * s, axis[1] / len * s, axis[2] / len * s, c],
        scale: [1.0; 3],
    };
    CsgNode::Transform(t, Box::new(boxp(1, [0.0; 3], [40.0, 25.0, 15.0])))
}

// ── Korpus & Registry ──────────────────────────────────────────────

pub const DEEP_CSG_SWEEP: [usize; 7] = [1, 2, 4, 8, 16, 32, 64];
pub const SMIN_CHAIN_SWEEP: [usize; 6] = [1, 2, 4, 8, 16, 32];

/// Die benannten Korpus-Szenen des Baseline-Laufs (Tiefen-Sweeps laufen
/// separat über [`DEEP_CSG_SWEEP`]/[`SMIN_CHAIN_SWEEP`]).
pub fn corpus() -> Vec<Scene> {
    vec![
        sphere_cal(),
        box_cal(),
        rotated_box(),
        thin_wall(0.05),
        thin_wall(0.02),
        thin_wall(0.01),
        thin_wall(0.005),
        tangent_spheres(0.10),
        tangent_spheres(0.01),
        tangent_spheres(0.0),
        tangent_spheres(-0.01),
        tangent_spheres(-0.25),
        deep_csg(8),
        deep_csg(64),
        smin_chain(2),
        smin_chain(16),
        twist_box(),
        bend_bar(),
        twist_thin_wall(),
        gyroid_box(),
        masked_blend_edge(),
    ]
}

/// Szene per Name (inkl. parametrischer Familien `deep_csg_<N>`,
/// `smin_chain_<N>`).
pub fn scene_by_name(name: &str) -> Option<Scene> {
    if let Some(n) = name.strip_prefix("deep_csg_") {
        return n.parse::<usize>().ok().filter(|&n| n >= 1 && n <= 256).map(deep_csg);
    }
    if let Some(n) = name.strip_prefix("smin_chain_") {
        return n.parse::<usize>().ok().filter(|&n| n >= 1 && n <= 256).map(smin_chain);
    }
    corpus().into_iter().find(|s| s.name == name)
}

fn fmt_permille(rel: f64) -> String {
    format!("{:04.1}pct", rel * 100.0).replace('.', "p")
}

fn fmt_signed_permille(rel: f64) -> String {
    let sign = if rel < 0.0 { "m" } else { "p" };
    format!("{}{:04.1}pct", sign, rel.abs() * 100.0).replace('.', "p")
}
