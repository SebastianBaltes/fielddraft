//! B7-Paritäts-Wächter: hält die verbliebenen Primitive-Mathe-Standorte
//! gegeneinander, damit sie nicht stillschweigend divergieren.
//!
//! Verglichen wird pro [`Primitive`]-Variante an einer Punktwolke
//! (innen / außen / nahe der Oberfläche, mehrere Parametrisierungen):
//!
//! 1. **f32-Referenz** [`sdf_primitive`] (= f32-Instanziierung der
//!    autoritativen [`sdf_primitive_generic`]),
//! 2. **Dual-Eval** über den `BoundedCsg`-Baum (`sdf_dual::sdf_gradient` —
//!    für die Sketch-Varianten der eigentliche Dual-Pfad auf Baum-Ebene),
//! 3. **Opcode-Interpreter** [`eval_opcodes_with_volumes`] über die
//!    kompilierten Opcodes (der CPU-Spiegel des GPU-Shaders; die
//!    Opcode-ENKODIERUNG selbst bleibt unangetastet).
//!
//! Zusätzlich: die Primitiv-AABB ([`Aabb::of_primitive`]) ist konservativ
//! (kein Punkt mit d < 0 außerhalb), und wo [`primitive_halfspaces`]
//! existiert, liegen auf die Oberfläche projizierte Punkte auf einem der
//! Halfspace-Carrier (grobe Plausibilität, und jede Fläche wird von
//! mindestens einem Punkt getroffen).
//!
//! Die Sample-Konstruktion in [`primitive_samples`] ist **Match-exhaustiv**:
//! eine neue `Primitive`-Variante bricht den Test-Compile, bis sie hier
//! Samples (und damit Paritäts-Deckung) bekommt.

use fielddraft_protocol::{BoolOp, FeatureId, MeshSdf, Primitive, Sketch2d, Sketch2dPrimitive, Transform};

use crate::body::build_bounded_for_body;
use crate::csg::{primitive_halfspaces, sdf_halfspace, Aabb, DecodedFeature, PrimitiveModifiers};
use crate::sdf_compiler::{compile_body, eval_opcodes_with_volumes};
use crate::sdf_dual::sdf_gradient;
use crate::sdf_primitive;

/// Ein Würfel-Mesh (±1) für die ImportedMesh-Samples.
fn cube_mesh() -> MeshSdf {
    #[rustfmt::skip]
    let positions: Vec<f32> = vec![
        -1.0, -1.0, -1.0,   1.0, -1.0, -1.0,   1.0, 1.0, -1.0,   -1.0, 1.0, -1.0,
        -1.0, -1.0,  1.0,   1.0, -1.0,  1.0,   1.0, 1.0,  1.0,   -1.0, 1.0,  1.0,
    ];
    #[rustfmt::skip]
    let indices: Vec<u32> = vec![
        0, 2, 1,  0, 3, 2, // -z
        4, 5, 6,  4, 6, 7, // +z
        0, 1, 5,  0, 5, 4, // -y
        3, 7, 6,  3, 6, 2, // +y
        0, 4, 7,  0, 7, 3, // -x
        1, 2, 6,  1, 6, 5, // +x
    ];
    MeshSdf::from_buffers(&positions, &indices)
}

fn triangle_sketch() -> Sketch2d {
    Sketch2d::Prim(Sketch2dPrimitive::Polygon {
        vertices: vec![[-3.0, -2.0], [4.0, -2.0], [0.0, 3.0]],
    })
}

fn circle_sketch(cx: f32, cy: f32, r: f32) -> Sketch2d {
    Sketch2d::Prim(Sketch2dPrimitive::Circle { center: [cx, cy], radius: r })
}

/// Mehrere Parametrisierungen je Variante. Der `match` am Ende ist der
/// Exhaustivitäts-Wächter: eine neue [`Primitive`]-Variante schlägt hier
/// als Compile-Fehler auf, bis sie Samples bekommt.
fn primitive_samples() -> Vec<Primitive> {
    let samples = vec![
        Primitive::Box { center: [0.0; 3], half_extents: [2.0, 2.0, 2.0] },
        Primitive::Box { center: [1.0, -2.0, 0.5], half_extents: [3.0, 1.0, 2.0] },
        Primitive::Sphere { center: [0.0; 3], radius: 2.0 },
        Primitive::Sphere { center: [1.5, 0.5, -1.0], radius: 0.75 },
        Primitive::Cylinder { center: [0.0; 3], radius: 2.0, half_height: 3.0 },
        Primitive::Cylinder { center: [-1.0, 2.0, 0.5], radius: 0.5, half_height: 5.0 },
        Primitive::Extrude { sketch: triangle_sketch(), half_depth_z: 2.0, center: [0.0; 3] },
        Primitive::Extrude {
            sketch: circle_sketch(1.0, 1.0, 2.0),
            half_depth_z: 0.5,
            center: [2.0, 0.0, 1.0],
        },
        Primitive::Revolve { sketch: circle_sketch(8.0, 0.0, 2.0), center: [0.0; 3] },
        Primitive::Helix {
            sketch: circle_sketch(6.0, 0.0, 1.0),
            center: [0.0; 3],
            pitch: 3.0,
            turns: 4.0,
            left_handed: false,
            taper: 0.0,
            start_angle: 0.0,
        },
        Primitive::Helix {
            sketch: circle_sketch(5.0, 0.0, 0.8),
            center: [0.0; 3],
            pitch: 2.5,
            turns: 2.5,
            left_handed: true,
            taper: 0.05,
            start_angle: 0.7,
        },
        Primitive::Torus { center: [0.0; 3], major_radius: 5.0, minor_radius: 1.5 },
        Primitive::Torus { center: [2.0, 1.0, 0.0], major_radius: 3.0, minor_radius: 0.5 },
        Primitive::Gyroid { cell_size: 2.0, thickness: 0.3 },
        Primitive::Thread { nominal_radius: 4.0, pitch: 1.5, depth: 0.5, half_height: 5.0 },
        Primitive::Cone { r_bottom: 3.0, r_top: 1.0, half_height: 2.0 },
        Primitive::Cone { r_bottom: 2.0, r_top: 0.0, half_height: 3.0 },
        Primitive::Capsule { radius: 1.5, half_height: 2.0 },
        Primitive::HexPrism { apothem: 2.0, half_height: 1.5 },
        Primitive::ImportedMesh { mesh: Box::new(cube_mesh()) },
        Primitive::Ellipsoid { radii: [3.0, 1.5, 2.0] },
        Primitive::CustomSdf {
            source: "sin(x)*cos(y) + sin(y)*cos(z) + sin(z)*cos(x) - 0.2".into(),
            ops: crate::sdf_expr::compile_expr(
                "sin(x)*cos(y) + sin(y)*cos(z) + sin(z)*cos(x) - 0.2",
            )
            .expect("valid parity expression"),
            lipschitz: 3.0,
            half_extents: [2.5, 2.5, 2.5],
        },
    ];
    // ── Exhaustivitäts-Wächter (KEIN Wildcard-Arm!) ────────────────────
    for p in &samples {
        match p {
            Primitive::Box { .. }
            | Primitive::Sphere { .. }
            | Primitive::Cylinder { .. }
            | Primitive::Extrude { .. }
            | Primitive::Torus { .. }
            | Primitive::HexPrism { .. }
            | Primitive::Ellipsoid { .. }
            | Primitive::Capsule { .. }
            | Primitive::Cone { .. }
            | Primitive::Thread { .. }
            | Primitive::ImportedMesh { .. }
            | Primitive::Gyroid { .. }
            | Primitive::Revolve { .. }
            | Primitive::Helix { .. }
            | Primitive::CustomSdf { .. } => {}
        }
    }
    samples
}

fn feature_for(prim: &Primitive) -> DecodedFeature {
    DecodedFeature::from_parts(
        FeatureId::from_u32(1),
        "parity".into(),
        BoolOp::New,
        prim.clone(),
        Transform::IDENTITY,
        PrimitiveModifiers::NONE,
        vec![],
    )
}

/// Punktwolke: 6×6×6-Gitter über die um 30 % (mindestens 0.5 mm) erweiterte
/// Primitiv-AABB — enthält Innen-, Außen- und oberflächennahe Punkte. Ein
/// raumfüllendes Primitiv (Gyroid, leere AABB) nutzt eine feste Box.
fn sample_points(prim: &Primitive) -> Vec<[f32; 3]> {
    let bb = if prim.is_space_filling() {
        Aabb { min: [-3.0; 3], max: [3.0; 3] }
    } else {
        Aabb::of_primitive(prim)
    };
    assert!(!bb.is_empty(), "sample AABB must not be empty for {prim:?}");
    let size = bb.size();
    let margin = (0.3 * size[0].max(size[1]).max(size[2])).max(0.5);
    let bb = bb.expanded(margin);
    // Ungerade Auflösung: das Gitter enthält Achsen-/Mittelpunkte, sodass
    // Projektionen auch kleine Deckflächen (Kegelstumpf-Top) treffen.
    const N: usize = 7;
    let mut pts = Vec::with_capacity(N * N * N);
    for i in 0..N {
        for j in 0..N {
            for k in 0..N {
                let f = |t: usize, lo: f32, hi: f32| {
                    lo + (hi - lo) * (t as f32 + 0.5) / N as f32
                };
                pts.push([
                    f(i, bb.min[0], bb.max[0]),
                    f(j, bb.min[1], bb.max[1]),
                    f(k, bb.min[2], bb.max[2]),
                ]);
            }
        }
    }
    pts
}

/// (Toleranz gegen Dual-Eval, Toleranz gegen Opcode-Eval), jeweils
/// skaliert mit `1 + |d_ref|`.
fn tolerances(prim: &Primitive) -> (f32, f32) {
    match prim {
        // Baked-Volume (64³, trilinear) — auflösungsbegrenzt grob.
        Primitive::ImportedMesh { .. } => (1e-3, 0.3),
        // Sketch-getragen: BVH-Sketch vs. naive Sketch-SDF plus
        // y-monotone Pfad-Records im Opcode-Strom.
        Primitive::Extrude { .. } | Primitive::Revolve { .. } | Primitive::Helix { .. } => {
            (2e-3, 2e-3)
        }
        _ => (1e-4, 1e-4),
    }
}

/// Kernvergleich: f32-Referenz ≈ Dual-Wert ≈ Opcode-Interpreter, für jede
/// Variante an der ganzen Punktwolke.
#[test]
fn primitive_eval_parity_ref_dual_opcodes() {
    for prim in primitive_samples() {
        let f = feature_for(&prim);
        let bounded = build_bounded_for_body(std::slice::from_ref(&f), &[0]);
        let compiled = compile_body(&bounded);
        let (tol_dual, tol_ops) = tolerances(&prim);
        for p in sample_points(&prim) {
            let d_ref = sdf_primitive(&prim, p);
            let d_dual = sdf_gradient(&bounded, p).0;
            let d_ops = eval_opcodes_with_volumes(&compiled.opcodes, p, &compiled.volumes);
            let scale = 1.0 + d_ref.abs();
            assert!(
                (d_ref - d_dual).abs() <= tol_dual * scale,
                "dual eval diverges for {prim:?} at {p:?}: ref={d_ref} dual={d_dual}"
            );
            // Baked-Volume-Sonderfall: außerhalb der Mesh-AABB clampt der
            // Texture-Sampler (wie der GPU-Shader) auf den Randwert — eine
            // bewusste, march-sichere UNTERschätzung. Dort prüfen wir nur,
            // dass der Interpreter keinen Innen-Punkt behauptet.
            if let Primitive::ImportedMesh { mesh } = &prim {
                let inside_aabb = (0..3)
                    .all(|a| p[a] >= mesh.aabb_min[a] - 0.05 && p[a] <= mesh.aabb_max[a] + 0.05);
                if inside_aabb {
                    assert!(
                        (d_ref - d_ops).abs() <= tol_ops * scale,
                        "opcode eval diverges for imported mesh at {p:?}: ref={d_ref} ops={d_ops}"
                    );
                } else {
                    assert!(
                        d_ops > -0.1,
                        "opcode eval claims interior OUTSIDE the mesh AABB at {p:?}: \
                         ref={d_ref} ops={d_ops}"
                    );
                }
                continue;
            }
            assert!(
                (d_ref - d_ops).abs() <= tol_ops * scale,
                "opcode eval diverges for {prim:?} at {p:?}: ref={d_ref} ops={d_ops}"
            );
        }
    }
}

/// Die Primitiv-AABB ist konservativ: kein Punkt der Wolke mit d < 0 liegt
/// außerhalb (raumfüllende Primitive haben bewusst eine leere AABB und sind
/// ausgenommen).
#[test]
fn primitive_aabb_is_conservative() {
    let inside = |bb: &Aabb, p: [f32; 3], slack: f32| {
        (0..3).all(|a| p[a] >= bb.min[a] - slack && p[a] <= bb.max[a] + slack)
    };
    for prim in primitive_samples() {
        if prim.is_space_filling() {
            continue;
        }
        let bb = Aabb::of_primitive(&prim);
        for p in sample_points(&prim) {
            let d = sdf_primitive(&prim, p);
            if d < -1e-4 {
                assert!(
                    inside(&bb, p, 1e-4),
                    "interior point {p:?} (d={d}) lies OUTSIDE the AABB of {prim:?} — \
                     Aabb::of_primitive is not conservative"
                );
            }
        }
    }
}

/// Wo eine Halfspace-Zerlegung existiert (Box/Cylinder/Sphere/Torus/Cone):
/// auf die Oberfläche projizierte Punkte liegen auf einem der analytischen
/// Carrier (|Halfspace-Distanz| ≈ 0), und jede Fläche wird von mindestens
/// einem Punkt getroffen (Ausnahme: die degenerierte Deckfläche eines
/// spitzen Kegels, r_top ≈ 0 — ein einzelner Punkt, den die Projektion
/// praktisch nie exakt trifft).
#[test]
fn primitive_halfspace_carriers_match_surface() {
    for prim in primitive_samples() {
        let Some(hs) = primitive_halfspaces(&prim) else { continue };
        let f = feature_for(&prim);
        let bounded = build_bounded_for_body(std::slice::from_ref(&f), &[0]);
        let mut face_hits = vec![0usize; hs.faces().len()];
        let mut projected = 0usize;
        for p0 in sample_points(&prim) {
            // Auf die Oberfläche projizieren (Gradientenabstieg, |∇| ≈ 1).
            let mut p = p0;
            let mut on_surface = false;
            for _ in 0..6 {
                let (d, g) = sdf_gradient(&bounded, p);
                let gl = (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt();
                if !d.is_finite() || !gl.is_finite() || gl < 1e-6 {
                    break;
                }
                p = [p[0] - d * g[0] / gl, p[1] - d * g[1] / gl, p[2] - d * g[2] / gl];
                if sdf_primitive(&prim, p).abs() < 1e-3 {
                    on_surface = true;
                    break;
                }
            }
            if !on_surface {
                continue; // Projektion konvergiert nicht (z.B. Sattel) — nächster Punkt.
            }
            projected += 1;
            // Auf welchem Carrier liegt der Punkt?
            let (best, dist) = hs
                .faces()
                .iter()
                .enumerate()
                .map(|(i, face)| {
                    let lp = [
                        p[0] - hs.center[0] - face.local_offset[0],
                        p[1] - hs.center[1] - face.local_offset[1],
                        p[2] - hs.center[2] - face.local_offset[2],
                    ];
                    (i, sdf_halfspace(&face.half_space, lp).abs())
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            assert!(
                dist < 5e-3,
                "surface point {p:?} of {prim:?} lies on NO half-space carrier \
                 (nearest face {best}, |d|={dist})"
            );
            face_hits[best] += 1;
        }
        assert!(
            projected > 20,
            "too few surface projections converged for {prim:?} ({projected})"
        );
        let degenerate_cone_cap = matches!(&prim, Primitive::Cone { r_top, .. } if *r_top < 0.05);
        if !degenerate_cone_cap {
            for (i, hits) in face_hits.iter().enumerate() {
                assert!(
                    *hits > 0,
                    "half-space face {i} of {prim:?} was never the nearest carrier — \
                     wrong surface/offset in primitive_halfspaces?"
                );
            }
        }
    }
}

