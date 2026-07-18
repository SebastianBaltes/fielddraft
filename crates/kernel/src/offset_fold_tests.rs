//! Unit-Tests des Erosions-Blatt-Einfaltens (offset_tree Phase A2, #384).
//! Ergänzt die Pass-/Integrations-Tests in `csg_tests.rs`.

use super::*;

fn tp_from(pts: &[(f64, f64, f64)], tags: &[u32]) -> TaggedPline {
    let mut tp = TaggedPline::new();
    for (p, t) in pts.iter().zip(tags.iter()) {
        tp.push(p.0, p.1, p.2, *t);
    }
    tp
}

/// Invertierung: Tags müssen der Segment-Geometrie folgen (Segment-
/// Mittelpunkte tragen vor und nach `invert` dasselbe Tag).
#[test]
fn invert_keeps_tags_on_their_geometry() {
    // Rechteck mit einer Bogenkante, Tags 0..3.
    let b = (std::f64::consts::PI / 8.0).tan();
    let mut tp = tp_from(
        &[(0.0, 0.0, 0.0), (4.0, 0.0, b), (4.0, 2.0, 0.0), (0.0, 2.0, 0.0)],
        &[0, 1, 2, 3],
    );
    let n = tp.pl.vertex_count();
    let before: Vec<([f64; 2], u32)> = (0..n)
        .map(|i| (pline_seg_mid(&tp.pl, i), tp.src[i]))
        .collect();
    tp.invert();
    for (mid, tag) in &before {
        // Finde das Segment mit demselben Mittelpunkt im invertierten Loop.
        let mut found = None;
        for i in 0..n {
            let m = pline_seg_mid(&tp.pl, i);
            if dist2(m, *mid) < 1e-9 {
                found = Some(tp.src[i]);
                break;
            }
        }
        assert_eq!(found, Some(*tag), "Tag wanderte bei invert()");
    }
}

/// Boolean-Restitch: Rechteck ∖ Randkreis — Provenance je Segment kommt
/// aus der richtigen Quelle (Kreissegmente tragen das Kreis-Tag).
#[test]
fn boolean_restitch_carries_source_tags() {
    let rect = tp_from(
        &[(0.0, 0.0, 0.0), (2.0, 0.0, 0.0), (2.0, 1.0, 0.0), (0.0, 1.0, 0.0)],
        &[10, 11, 12, 13],
    );
    let circle = tp_from(&[(2.4, 0.5, 1.0), (1.6, 0.5, 1.0)], &[42, 42]);
    let (pos, neg) = bool_pair(&rect, &circle, BooleanOp::Not).expect("boolean");
    assert_eq!(pos.len(), 1);
    assert!(neg.is_empty());
    let r = &pos[0];
    assert!(r.area() > 0.0, "Ergebnis muss CCW normalisiert sein");
    let n = r.pl.vertex_count();
    let mut saw_circle_tag = false;
    for i in 0..n {
        let mid = pline_seg_mid(&r.pl, i);
        let tag = r.src[i];
        // Segment-Mittelpunkt liegt auf SEINER Quelle.
        let src = if tag == 42 { &circle } else { &rect };
        let mut sources = Vec::new();
        collect_seg_geoms(src, &mut sources);
        let d = sources
            .iter()
            .map(|(g, _)| seg_geom_dist(g, mid))
            .fold(f64::MAX, f64::min);
        assert!(d < 1e-9, "Segment {i} (Tag {tag}) liegt {d} neben seiner Quelle");
        saw_circle_tag |= tag == 42;
    }
    assert!(saw_circle_tag, "die Halbkreiskante muss das Kreis-Tag tragen");
}

/// Shape-Erosion + Rückprojektion: C-Profil spaltet in zwei Loops, jede
/// bei Abstand exakt δ zur Quelle; Tags bleiben eindeutige Quellgruppen.
#[test]
fn erosion_backprojection_tags_split_loops() {
    let c_profile = tp_from(
        &[
            (0.0, 0.0, 0.0),
            (3.0, 0.0, 0.0),
            (3.0, 1.2, 0.0),
            (0.8, 1.2, 0.0),
            (0.8, 1.8, 0.0),
            (3.0, 1.8, 0.0),
            (3.0, 3.0, 0.0),
            (0.0, 3.0, 0.0),
        ],
        &[0, 1, 2, 3, 4, 5, 6, 7],
    );
    let region = vec![Component { outer: c_profile, holes: Vec::new() }];
    let eroded = offset_region(&region, 0.5).expect("erosion");
    assert_eq!(eroded.len(), 2, "C-Profil muss in zwei Komponenten spalten");
    let mut saw_join = false;
    for c in &eroded {
        assert!(c.holes.is_empty());
        assert!(c.outer.area() > 0.0);
        // Tags sind Quellgruppen (< 8) oder der JOIN-Marker der
        // Reflex-Ecken-Verrundungsbögen.
        assert!(c.outer.src.iter().all(|&t| t < 8 || t == JOIN));
        saw_join |= c.outer.src.iter().any(|&t| t == JOIN);
    }
    assert!(saw_join, "Reflex-Slot-Ecke muss einen Join-Bogen erzeugen");
}

/// Sektor-Lowering: CW-Sweep wird CCW normalisiert, Tags folgen.
#[test]
fn sector_prim_normalises_to_ccw() {
    let prim = Sketch2dPrimitive::Sector {
        center: [1.0, 2.0],
        radius: 3.0,
        start_angle: 0.5,
        sweep_angle: -1.2,
    };
    let tp = prim_to_loop(&prim, 7).expect("kein Verweigern").expect("nicht leer");
    assert!(tp.area() > 0.0);
    let mut tags = tp.src.clone();
    tags.sort_unstable();
    tags.dedup();
    assert_eq!(tags, vec![7, 8, 9]);
}

/// QuadBezier-Profile sind vom Fold ausgeschlossen (A1: kein
/// cavalier-Gegenstück; Biarc ist ein Folgeticket).
#[test]
fn quad_bezier_profile_refuses() {
    let p = Sketch2dPrimitive::Path {
        segments: vec![
            SketchSegment::Line { start: [0.0, 0.0], end: [2.0, 0.0] },
            SketchSegment::QuadBezier { p0: [2.0, 0.0], p1: [2.0, 2.0], p2: [0.0, 2.0] },
            SketchSegment::Line { start: [0.0, 2.0], end: [0.0, 0.0] },
        ],
        closed: true,
    };
    assert!(prim_to_loop(&p, 0).is_none());
    assert!(erode_profile(&Sketch2d::Prim(p), None, 0.2, false).is_none());
}

/// Offene (achs-gestrippte) Pfade werden verweigert — die Kette hat
/// Lücken, cavalier bekäme keine geschlossene Eingabe.
#[test]
fn gapped_path_refuses() {
    let p = Sketch2dPrimitive::Path {
        segments: vec![
            SketchSegment::Line { start: [0.0, -1.0], end: [2.0, -1.0] },
            SketchSegment::Line { start: [2.0, -1.0], end: [2.0, 1.0] },
            SketchSegment::Line { start: [2.0, 1.0], end: [0.0, 1.0] },
            // Lücke: kein Schluss-Segment zurück zu (0,−1).
        ],
        closed: true,
    };
    assert!(prim_to_loop(&p, 0).is_none());
}
