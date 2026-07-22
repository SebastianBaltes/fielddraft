//! Design-Spike „Folgeprogramm (c)" (#398/#384): nicht-konvexe / facettierte
//! koplanare Kontaktflächen via `cavalier_contours`-Polygon-Booleans.
//!
//! MACHBARKEITS-BELEG, KEIN Produktionscode. Der Kontakt-Prisma-/∂P-Rim-Pfad
//! (`offset_channel.rs`, `offset_rim.rs`) extrahiert die Kontaktfläche P heute
//! ausschliesslich aus BOX-BOX-Kontakten (`box_contact_faces` + Sutherland–
//! Hodgman `clip_convex` auf konvexen Rechtecken). Nicht-konvexe/facettierte
//! Kontakte (L-Profil an Box, Extrude an Extrude) meldet die Capability ehrlich
//! als unkorrigierten Rest.
//!
//! Dieser Test zeigt, dass `cavalier_contours` 0.7 (bereits Dependency) den
//! Verschnitt zweier koplanarer, NICHT-konvexer Konturen exakt rechnet und die
//! mehreren Zusammenhangskomponenten des Kontakts P als getrennte `pos_plines`
//! liefert — die Grundoperation, die das Folgeprogramm (c) verallgemeinert.
//!
//! Design-SSOT: `docs/research/sdf-metricity/FC-NONCONVEX-CONTACT-DESIGN.md`.

use cavalier_contours::pline_closed;
use cavalier_contours::polyline::{BooleanOp, PlineSource, Polyline};

/// Signierte Fläche (Shoelace) über die Polyline-Vertices (Linien-Näherung;
/// die Testkonturen tragen keine Bögen).
fn signed_area(pl: &Polyline<f64>) -> f64 {
    let n = pl.vertex_count();
    let mut a = 0.0;
    for i in 0..n {
        let p = pl.at(i);
        let q = pl.at((i + 1) % n);
        a += p.x * q.y - q.x * p.y;
    }
    0.5 * a
}

/// Bogen-tragende Vertices (bulge ≠ 0) — bei geraden Kontakt-Konturen null,
/// belegt aber, dass die Repräsentation Bögen mitführen KANN.
fn arc_vertices(pl: &Polyline<f64>) -> usize {
    (0..pl.vertex_count())
        .filter(|&i| pl.at(i).bulge.abs() > 1e-12)
        .count()
}

/// L-förmige Kontur (nicht-konvex, sechs Ecken) im ersten Quadranten:
///
///   (0,0) → (6,0) → (6,2) → (2,2) → (2,6) → (0,6) → schliesst.
///
/// Ein Quadrat [0,6]² mit dem oberen-rechten 4×4-Block bei [2,6]×[2,6]
/// herausgenommen. Reflex-Ecke bei (2,2).
fn l_shape() -> Polyline<f64> {
    pline_closed![
        (0.0, 0.0, 0.0),
        (6.0, 0.0, 0.0),
        (6.0, 2.0, 0.0),
        (2.0, 2.0, 0.0),
        (2.0, 6.0, 0.0),
        (0.0, 6.0, 0.0),
    ]
}

/// Der Verschnitt einer L-Form mit einem sie überragenden Rechteck ist die
/// L-Form selbst (enthaltener Kontakt) — der nicht-konvexe Analog-Fall zum
/// Box-Stack (P = kleinerer Querschnitt, vollständig im Über-Stander).
#[test]
fn l_shape_contained_in_rect_is_l_shape() {
    let l = l_shape();
    // Über-Stander: Rechteck [-1,7]×[-1,7] ⊃ L.
    let rect = pline_closed![
        (-1.0, -1.0, 0.0),
        (7.0, -1.0, 0.0),
        (7.0, 7.0, 0.0),
        (-1.0, 7.0, 0.0),
    ];
    let res = l.boolean(&rect, BooleanOp::And);
    assert_eq!(res.pos_plines.len(), 1, "enthaltener Kontakt: EINE Komponente");
    assert_eq!(res.neg_plines.len(), 0, "kein Loch");
    let p = &res.pos_plines[0].pline;
    // Fläche der L-Form = 6·6 − 4·4 = 20.
    let area = signed_area(p).abs();
    assert!((area - 20.0).abs() < 1e-6, "L-Fläche 20 erwartet, got {area}");
    assert_eq!(arc_vertices(p), 0, "gerade Kontur, keine Bögen");
}

/// KERNBELEG der Komponenten-Zerlegung: zwei koplanare nicht-konvexe Konturen,
/// deren Verschnitt in ZWEI disjunkte Kontaktinseln zerfällt. Eine L-Form
/// gegen ihre um die Diagonale gespiegelte Kopie (⌐ gegen ⌙) überlappt an den
/// beiden dünnen Schenkel-Enden, nicht in der Mitte → zwei Komponenten.
#[test]
fn two_ls_intersect_in_two_components() {
    // L wie oben (Schenkel entlang +x und +y aus der Ecke bei 0).
    let l1 = l_shape();
    // Zweite L: an der Geraden y = x gespiegelt UND so verschoben, dass sich
    // nur die zwei Schenkel-Enden überlappen. Wir nehmen ein „Γ": ein
    // horizontaler Balken oben + vertikaler Balken rechts, der die zwei
    // dünnen Enden von l1 (den +x-Fuss bei y∈[0,2] und den +y-Fuss bei
    // x∈[0,2]) trifft, aber die Reflex-Region aussen lässt.
    //
    //   l2: horizontaler Balken y∈[4,6] über x∈[0,6]  ∪  vertikaler Balken
    //       x∈[4,6] über y∈[0,6]  — als eine nicht-konvexe „⌐"-Kontur:
    //   (0,4)(4,4)(4,0)(6,0)(6,6)(0,6)
    let l2 = pline_closed![
        (0.0, 4.0, 0.0),
        (4.0, 4.0, 0.0),
        (4.0, 0.0, 0.0),
        (6.0, 0.0, 0.0),
        (6.0, 6.0, 0.0),
        (0.0, 6.0, 0.0),
    ];
    let res = l1.boolean(&l2, BooleanOp::And);
    // l1 (⌐ mit Füssen bei y∈[0,2] und x∈[0,2]) ∩ l2 (⌐ mit Balken bei
    // y∈[4,6] und x∈[4,6]) = zwei disjunkte Rechtecke:
    //   A: x∈[4,6], y∈[0,2]  (l1-x-Fuss ∩ l2-x-Balken)
    //   B: x∈[0,2], y∈[4,6]  (l1-y-Fuss ∩ l2-y-Balken)
    assert_eq!(
        res.pos_plines.len(),
        2,
        "zwei disjunkte Kontaktinseln erwartet, got {}",
        res.pos_plines.len()
    );
    let mut areas: Vec<f64> = res.pos_plines.iter().map(|s| signed_area(&s.pline).abs()).collect();
    areas.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for a in &areas {
        assert!((a - 4.0).abs() < 1e-6, "jede Insel 2×2=4, got {a}");
    }
    let total: f64 = areas.iter().sum();
    assert!((total - 8.0).abs() < 1e-6, "Gesamtkontakt 8, got {total}");
}

/// Verschnitt mit einem innenliegenden Abzug erzeugt ein LOCH (neg_pline) —
/// belegt, dass die Repräsentation mehrfach-zusammenhängende Kontakte
/// (Insel mit Loch) trägt: der nicht-konvexe Fall eines Kontakt-P mit
/// Innenrand, den ein konvexer Clip nie erzeugen kann.
#[test]
fn contact_with_hole_yields_neg_pline() {
    // Grosse Platte.
    let plate = pline_closed![
        (0.0, 0.0, 0.0),
        (10.0, 0.0, 0.0),
        (10.0, 10.0, 0.0),
        (0.0, 10.0, 0.0),
    ];
    // Innenliegender kreisförmiger Abzug (bulge = 1 → Halbkreise).
    let hole = pline_closed![(7.0, 5.0, 1.0), (3.0, 5.0, 1.0)];
    let ring = plate.boolean(&hole, BooleanOp::Not);
    assert_eq!(ring.pos_plines.len(), 1, "Aussenrand");
    assert_eq!(ring.neg_plines.len(), 1, "genau ein Loch");
    // Das Loch trägt ECHTE Bögen (bulge ≠ 0) — Bulge-Kontakte sind
    // repräsentierbar, auch wenn das Design sie (gekrümmte Kontakte)
    // ausdrücklich NICHT korrigiert.
    assert!(
        arc_vertices(&ring.neg_plines[0].pline) > 0,
        "Loch-Rand muss Bögen tragen (Bulge-Repräsentation)"
    );
}
