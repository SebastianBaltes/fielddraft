//! offset_tree Phase A2 (#384): analytisches Blatt-Einfalten von
//! **Erosions**-Offsets (`CsgNode::Offset(leaf, δ)` mit `δ < 0`).
//!
//! Nach `CsgNode::push_down_offsets` sitzen Offsets an Blättern und
//! Stopp-Knoten. Auf einem exakten Blatt ist `f − δ` für Dilation (`δ > 0`)
//! bereits das exakte Dilations-SDF; für Erosion ist es nur
//! nullmengen-exakt (Außenfeld unterschätzt an konvexen Ecken, Innenfeld
//! ok). Dieses Modul ersetzt den Erosions-Fall durch ein **neues Blatt
//! derselben Familie mit geschrumpften Parametern**, wo die Trägerklasse
//! erosions-abgeschlossen ist (docs/OFFSET-TREE-PLAN.md §5,
//! docs/OFFSET-TREE-AUDIT.md):
//!
//! * Kugel / Zylinder / Torus-Rohrradius → `r − |δ|` (Zylinder zusätzlich
//!   `half_height − |δ|`), Box → geschrumpfte Box, Kegel(-stumpf) →
//!   gleiche Wandneigung mit radial um `|δ|/cos α` eingerückter Wand und
//!   um `|δ|` eingerückten Deckeln (Erosion distribuiert exakt über die
//!   Halbraum-Intersektion des Primitivs).
//! * Extrude: Profil-Booleans und `Sketch2d::Offset` werden VOR dem Fold
//!   **geometrisch aufgelöst** (cavalier_contours, Auswahl-Spike
//!   docs/research/sdf-metricity/iter3-offset-tree/A1-OFFSET-LIB.md),
//!   dann exakt `Extrude(profil↓|δ|, half_height − |δ|)`.
//! * Revolve: Meridian-Profil-Erosion in (u,v); Gate: Profil (und
//!   Ergebnis) müssen strikt achsenfern bleiben (#390 — ein
//!   achsberührender Fold würde neue Phantom-Nullmengen erzeugen).
//!
//! **Identitätsvertrag** (Audit, Punkte 3–5): Der Fold ändert NIE die
//! Blatt-`FeatureId` und NIE den Sub-Face-Raum. Für Sketch-Sweeps wird die
//! `wall_subs`-Tabelle durch den 2D-Umbau **propagiert**: jede
//! Ergebnis-Boundary-Gruppe erbt die stabile Sub-Id ihrer Quellgruppe
//! (Booleans: `BooleanResultPline.subslices`; Offset: exakte
//! Rückprojektion — jedes Ergebnis-Segment liegt konstruktiv bei Abstand
//! genau `δ` zu genau einem Quellsegment, Join-Bögen erben vom im
//! Umlaufsinn vorangehenden Segment). Kein neuer Primitive-Variant.
//!
//! **Verweigerungs-Doktrin:** Wo der Fold nicht exakt oder die Provenance
//! nicht eindeutig ist (QuadBezier-Segmente, offene/selbstschneidende
//! Profile, Nesting-Tiefe > 2, Achsberührung, degenerierte Parameter,
//! Sub-Id-Kollisionen), gibt jede Funktion `None` zurück und der Aufrufer
//! lässt `f − δ` stehen — nie approximieren.
//!
//! Die Skalar-Arithmetik (`r − t`) läuft über dieselben Helfer wie
//! `csg::offset_halfspace_inward` (`offset_scalar_inward` /
//! `shrink_param`), damit gefaltete Blatt-Carrier bit-exakt zu den
//! verschobenen Carriern der verbleibenden Stopp-Knoten sind
//! (Koinzidenz-Leitregel: analytisch, nie Sampling-Epsilon).

use cavalier_contours::polyline::{
    BooleanOp, BooleanPlineSlice, BooleanResultInfo, PlineContainsResult, PlineSource,
    PlineSourceMut, Polyline,
};
use cavalier_contours::shape_algorithms::{Shape, ShapeOffsetOptions};

use fielddraft_protocol::sketch2d::{g1_group_segments, tangent_at_joint, PATH_G1_COS_THRESHOLD};
use fielddraft_protocol::Primitive;

use crate::csg::{offset_scalar_inward, shrink_param, BranchId, CsgNode};
use crate::sketch2d::{Sketch2d, Sketch2dPrimitive, SketchSegment};

/// Achsabstands-Gate für Revolve-Folds — gleiche Größenordnung wie
/// [`crate::sketch::REVOLVE_AXIS_EPS`], aber als eigenes f64-Gate: das
/// erodierte Profil muss STRIKT achsenfern sein.
const REVOLVE_FOLD_AXIS_EPS: f64 = 1e-4;

/// Provenance-Marker „Join-Bogen": das Segment liegt bei Abstand δ zu
/// ZWEI Quellsegmenten gleichzeitig (= zum gemeinsamen Quell-Vertex) —
/// ein vom Offset NEU erzeugter Verrundungsbogen ohne Quellwand.
const JOIN: u32 = u32::MAX;

/// Sub-Id-Band für Join-Bogen-Wände. Ein Join-Bogen ist eine GENUIN NEUE
/// Wand (wie ein vom Nutzer gezeichneter Fillet-Bogen) und braucht eine
/// eigene Branch-Identität: erbte er die Sub seiner Nachbarwand, würde
/// sein Zylinder-Carrier in der Surface-Registry den Plane-Carrier der
/// Nachbarwand überschreiben (gleicher Branch-Key). Die Subs werden
/// deterministisch in Ergebnis-Reihenfolge ab hier vergeben; das Band
/// liegt unter den reservierten Cap-Subs
/// ([`BranchId::EXTRUDE_CAP_BOTTOM_SUB`] = 0x7FE) und weit über realen
/// Sketch-Entity-Ordinalen. Kollidiert eine Original-Sub mit dem Band
/// (Sketch mit ≥ 0x700 Entities), wird der Fold verweigert.
const JOIN_ARC_SUB_BASE: u32 = 0x700;

// ─── Einstieg: ein Erosions-Offset über einem Blattknoten falten ─────────

/// Ergebnis eines Blatt-Fold-Versuchs (Phase C1 erweitert den A2-Fold um
/// den **parametrischen Kollaps**).
pub(crate) enum FoldOutcome {
    /// Neues Blatt derselben Familie mit geschrumpften Parametern.
    Folded(CsgNode),
    /// Die Erosion verschluckt das Blatt vollständig — parametrisch
    /// exakt entschieden (r−a ≤ 0, half_height−a ≤ 0, 2D-Profil
    /// verschwindet im cavalier-Offset). Der Aufrufer ersetzt das Blatt
    /// durch [`CsgNode::Empty`]; das ist topologisch die WAHRE Erosion,
    /// kein Fallback (Plan §7.1: keine Heuristik, kein `f − δ`, das auf
    /// inexakten Profil-Feldern Geisterkörper stehen ließe).
    Collapsed,
}

/// Ergebnis der Primitiv-Parameter-Erosion.
enum PrimFold {
    Shrunk(Primitive),
    Collapsed,
}

/// Versucht, `Offset(node, −a)` (Erosion um `a > 0`) als neues Blatt
/// derselben Familie darzustellen. `None` = verweigern (`f − δ` bleibt);
/// [`FoldOutcome::Collapsed`] = das Blatt verschwindet exakt (Phase C1).
pub(crate) fn try_fold_erosion(node: &CsgNode, a: f32) -> Option<FoldOutcome> {
    if !(a > 0.0) || !a.is_finite() {
        return None;
    }
    match node {
        CsgNode::Leaf { id, primitive } => match fold_primitive_erosion(primitive, a)? {
            PrimFold::Shrunk(p) => Some(FoldOutcome::Folded(CsgNode::Leaf {
                id: id.clone(),
                primitive: p,
            })),
            PrimFold::Collapsed => Some(FoldOutcome::Collapsed),
        },
        CsgNode::Extrude { id, sketch, half_height, wall_subs } => {
            // Erosion distribuiert exakt über die Prisma-Intersektion:
            // kollabiert der Höhen-Slab (hh − a ≤ 0), ist die wahre Erosion
            // leer — unabhängig davon, ob das Profil faltbar wäre.
            let Some(hh2) = shrink_param(*half_height, a) else {
                return Some(FoldOutcome::Collapsed);
            };
            let folded = erode_profile(sketch, wall_subs.as_deref().map(|v| v.as_slice()), a, false)?;
            Some(match folded {
                ErodedProfile::Vanished => FoldOutcome::Collapsed,
                ErodedProfile::InFamily(s) => FoldOutcome::Folded(CsgNode::Extrude {
                    id: id.clone(),
                    sketch: Box::new(s),
                    half_height: hh2,
                    wall_subs: wall_subs.clone(),
                }),
                ErodedProfile::Rebuilt(s, ws) => FoldOutcome::Folded(CsgNode::Extrude {
                    id: id.clone(),
                    sketch: Box::new(s),
                    half_height: hh2,
                    wall_subs: Some(std::sync::Arc::new(ws)),
                }),
            })
        }
        CsgNode::Revolve { id, sketch, wall_subs } => {
            let folded = erode_profile(sketch, wall_subs.as_deref().map(|v| v.as_slice()), a, true)?;
            Some(match folded {
                ErodedProfile::Vanished => FoldOutcome::Collapsed,
                ErodedProfile::InFamily(s) => FoldOutcome::Folded(CsgNode::Revolve {
                    id: id.clone(),
                    sketch: Box::new(s),
                    wall_subs: wall_subs.clone(),
                }),
                ErodedProfile::Rebuilt(s, ws) => FoldOutcome::Folded(CsgNode::Revolve {
                    id: id.clone(),
                    sketch: Box::new(s),
                    wall_subs: Some(std::sync::Arc::new(ws)),
                }),
            })
        }
        _ => None,
    }
}

/// Erosions-Fold der analytischen Primitive (Audit-Familienliste — KEIN
/// neuer Variant, keine Erweiterung auf ungelistete Familien).
/// `PrimFold::Collapsed`, wenn ein tragender Parameter unter die
/// Degenerationsgrenze schrumpft — dort ist die WAHRE Erosion die leere
/// Menge (Kugel/Zylinder/Box/Torus-Rohr sind Intersektionen bzw.
/// Sweeps, deren Minkowski-Erosion mit dem Parameter kollabiert).
fn fold_primitive_erosion(prim: &Primitive, a: f32) -> Option<PrimFold> {
    // Kollaps-bewusster Schrumpf: `None` von `shrink_param` heißt hier
    // IMMER Kollaps (der Parameter trägt die letzte Ausdehnung der Familie).
    macro_rules! shrink_or_collapse {
        ($v:expr) => {
            match shrink_param($v, a) {
                Some(s) => s,
                None => return Some(PrimFold::Collapsed),
            }
        };
    }
    match prim {
        Primitive::Sphere { center, radius } => Some(PrimFold::Shrunk(Primitive::Sphere {
            center: *center,
            radius: shrink_or_collapse!(*radius),
        })),
        Primitive::Cylinder { center, radius, half_height } => {
            Some(PrimFold::Shrunk(Primitive::Cylinder {
                center: *center,
                radius: shrink_or_collapse!(*radius),
                half_height: shrink_or_collapse!(*half_height),
            }))
        }
        Primitive::Box { center, half_extents } => Some(PrimFold::Shrunk(Primitive::Box {
            center: *center,
            half_extents: [
                shrink_or_collapse!(half_extents[0]),
                shrink_or_collapse!(half_extents[1]),
                shrink_or_collapse!(half_extents[2]),
            ],
        })),
        Primitive::Torus { center, major_radius, minor_radius } => {
            Some(PrimFold::Shrunk(Primitive::Torus {
                center: *center,
                major_radius: *major_radius,
                minor_radius: shrink_or_collapse!(*minor_radius),
            }))
        }
        // Kegelstumpf: Erosion = Intersektion der erodierten Halbräume.
        // Deckel rücken um `a` ein (`hh − a`), die Wand rückt senkrecht um
        // `a` ein → radial um `a·slant/(2hh)`; die Radien an den NEUEN
        // Deckelhöhen ±(hh−a) folgen der unveränderten Wandgeraden. Die
        // Wandneigung bleibt (reell) exakt erhalten.
        Primitive::Cone { r_bottom, r_top, half_height } => {
            let hh = *half_height;
            if !(hh > 0.0) {
                return None;
            }
            let hh2 = shrink_or_collapse!(hh);
            let dr = r_bottom - r_top;
            let two_hh = 2.0 * hh;
            let slant = (dr * dr + two_hh * two_hh).sqrt();
            let radial = a * slant / two_hh;
            let tilt = a * dr / two_hh;
            let r_b2 = offset_scalar_inward(r_bottom - tilt, radial);
            let r_t2 = offset_scalar_inward(*r_top + tilt, radial);
            if r_b2 <= 1e-4 && r_t2 <= 1e-4 {
                // Beide Radien der Wandgeraden weg → Radialausdehnung über
                // die gesamte (bereits gültige) Resthöhe ≤ 0: leer.
                return Some(PrimFold::Collapsed);
            }
            if r_b2 <= 1e-4 || r_t2 <= 1e-4 {
                // Nur eine Seite kollabiert: die wahre Erosion ist ein
                // KÜRZERER Kegel mit Spitze — nicht in-family darstellbar,
                // ohne die Deckelhöhen zu verschieben → verweigern
                // (`f − δ` am exakten Kegel-Blatt bleibt nullmengen-exakt).
                return None;
            }
            Some(PrimFold::Shrunk(Primitive::Cone {
                r_bottom: r_b2,
                r_top: r_t2,
                half_height: hh2,
            }))
        }
        // Alles andere (HexPrism/Ellipsoid/Capsule/Thread/Gyroid/Mesh/
        // Sweeps als Primitive): nicht im A2-Scope → verweigern.
        _ => None,
    }
}

// ─── 2D-Profil-Erosion (Extrude/Revolve) ─────────────────────────────────

/// Ergebnis von [`erode_profile`].
pub(crate) enum ErodedProfile {
    /// Gruppen-Struktur unverändert (Circle → r−a, Rectangle → he−a):
    /// bestehende `wall_subs` bleiben unverändert gültig.
    InFamily(Sketch2d),
    /// Profil geometrisch neu aufgebaut; zweite Komponente ist die
    /// propagierte `wall_subs`-Tabelle (neue Positionsgruppe → stabile
    /// Sub-Id; bei ursprünglich positionalen Subs die ORIGINAL-Gruppen-Ids,
    /// womit der persistierte Sub-Raum erhalten bleibt).
    Rebuilt(Sketch2d, Vec<u32>),
    /// Phase C1: das erodierte Profil ist LEER — cavalier_contours (bzw.
    /// der in-family-Schrumpf) hat den Kollaps topologisch exakt
    /// entschieden. Der Sweep-Körper verschwindet vollständig.
    Vanished,
}

/// Erodiert ein Sweep-Profil geometrisch um `a` (mm). `revolve_gate`
/// aktiviert das Achsabstands-Gate (#390).
pub(crate) fn erode_profile(
    sketch: &Sketch2d,
    wall_subs: Option<&[u32]>,
    a: f32,
    revolve_gate: bool,
) -> Option<ErodedProfile> {
    let a64 = a as f64;
    // In-family-Schnellpfade: Gruppenzahl und -reihenfolge bleiben exakt
    // erhalten, kein cavalier nötig. (Erosion hält konvexe Ecken scharf —
    // das geschrumpfte Rechteck ist die exakte Minkowski-Erosion.)
    if let Sketch2d::Prim(p) = sketch {
        match p {
            Sketch2dPrimitive::Circle { center, radius } => {
                // Achs-Gate auf dem ORIGINAL: berührt das Profil die Achse,
                // ist die Meridian-Erosion falsch (die Achsseite ist innen,
                // nicht Rand) — verweigern, f − δ bleibt.
                if revolve_gate
                    && (center[0] as f64 - *radius as f64) <= REVOLVE_FOLD_AXIS_EPS
                {
                    return None;
                }
                let Some(r2) = shrink_param(*radius, a) else {
                    return Some(ErodedProfile::Vanished); // C1: r − a ≤ 0
                };
                return Some(ErodedProfile::InFamily(Sketch2d::Prim(
                    Sketch2dPrimitive::Circle { center: *center, radius: r2 },
                )));
            }
            Sketch2dPrimitive::Rectangle { center, half_extents } => {
                if revolve_gate
                    && (center[0] as f64 - half_extents[0] as f64) <= REVOLVE_FOLD_AXIS_EPS
                {
                    return None;
                }
                let (Some(hx), Some(hy)) = (
                    shrink_param(half_extents[0], a),
                    shrink_param(half_extents[1], a),
                ) else {
                    return Some(ErodedProfile::Vanished); // C1: he − a ≤ 0
                };
                return Some(ErodedProfile::InFamily(Sketch2d::Prim(
                    Sketch2dPrimitive::Rectangle { center: *center, half_extents: [hx, hy] },
                )));
            }
            _ => {}
        }
    }

    // Allgemeiner Pfad: Baum → Region (Booleans/Offset geometrisch
    // auflösen) → Shape-Erosion → Profil + Tabelle neu aufbauen.
    let mut base = 0u32;
    let region = resolve_region(sketch, &mut base)?;
    if region.is_empty() {
        // C1: das Original-Profil löst bereits zur leeren Region auf —
        // der Sweep trägt kein Material, die Erosion erst recht nicht.
        return Some(ErodedProfile::Vanished);
    }
    if revolve_gate && !region_off_axis(&region) {
        return None;
    }
    let eroded = offset_region(&region, a64)?;
    if eroded.is_empty() {
        // C1: cavalier hat den Kollaps topologisch exakt entschieden
        // (Insel verschwindet, Profil unter 2a Breite): leeres Profil.
        return Some(ErodedProfile::Vanished);
    }
    if revolve_gate && !region_off_axis(&eroded) {
        return None;
    }
    let (profile, table) = region_to_profile(&eroded, wall_subs)?;
    Some(ErodedProfile::Rebuilt(profile, table))
}

fn region_off_axis(region: &[Component]) -> bool {
    for c in region {
        for tp in std::iter::once(&c.outer).chain(c.holes.iter()) {
            match tp.pl.extents() {
                Some(e) if e.min_x > REVOLVE_FOLD_AXIS_EPS => {}
                _ => return false,
            }
        }
    }
    true
}

// ─── Region-Darstellung ──────────────────────────────────────────────────

/// Geschlossene CCW-Polyline mit per-Segment-Provenance: `src[i]` ist die
/// globale Boundary-Gruppen-Id des ORIGINAL-Profilbaums, aus der Segment
/// `i` (Vertex `i` → `i+1`, wrapping) stammt.
#[derive(Clone)]
struct TaggedPline {
    pl: Polyline<f64>,
    src: Vec<u32>,
}

/// Materialkomponente: eine Außenkontur mit strikt innenliegenden,
/// paarweise disjunkten Löchern (Nesting-Tiefe ≤ 2; tiefere Verschachtelung
/// wird verweigert).
#[derive(Clone)]
struct Component {
    outer: TaggedPline,
    holes: Vec<TaggedPline>,
}

type Region = Vec<Component>;

impl TaggedPline {
    fn new() -> Self {
        let mut pl = Polyline::new();
        pl.set_is_closed(true);
        TaggedPline { pl, src: Vec::new() }
    }

    fn push(&mut self, x: f64, y: f64, bulge: f64, tag: u32) {
        // Doppelte Punkte (Slice-Nähte) verschlucken — ein Null-Segment
        // trägt keine Geometrie, sein Tag entfällt mit ihm.
        if let Some(last) = (self.pl.vertex_count() > 0).then(|| self.pl.at(self.pl.vertex_count() - 1)) {
            if (last.x - x).abs() < 1e-10 && (last.y - y).abs() < 1e-10 {
                // Position identisch: Bulge/Tag des NEUEN Segments gewinnen.
                let n = self.pl.vertex_count();
                self.pl
                    .set_vertex(n - 1, last.with_bulge(bulge));
                *self.src.last_mut().unwrap() = tag;
                return;
            }
        }
        self.pl.add(x, y, bulge);
        self.src.push(tag);
    }

    /// Schließ-Naht säubern: fällt der letzte Vertex auf den ersten, wird
    /// er entfernt (das Schluss-Segment übernimmt seine Rolle).
    fn seal(&mut self) {
        let n = self.pl.vertex_count();
        if n >= 2 {
            let first = self.pl.at(0);
            let last = self.pl.at(n - 1);
            if (first.x - last.x).abs() < 1e-10 && (first.y - last.y).abs() < 1e-10 {
                self.pl.remove(n - 1);
                self.src.pop();
            }
        }
    }

    fn area(&self) -> f64 {
        self.pl.area()
    }

    /// CCW normalisieren; Tags folgen der Segment-Geometrie.
    fn ensure_ccw(&mut self) {
        if self.area() < 0.0 {
            self.invert();
        }
    }

    fn invert(&mut self) {
        let n = self.pl.vertex_count();
        self.pl.invert_direction_mut();
        // Neues Segment j läuft über die Geometrie des alten Segments
        // (n − 2 − j) mod n (Herleitung: invert_direction_mut kehrt die
        // Vertexfolge um und verschiebt die Bulges um eins).
        let old = self.src.clone();
        for (j, slot) in self.src.iter_mut().enumerate() {
            let idx = (n as i64 - 2 - j as i64).rem_euclid(n as i64) as usize;
            *slot = old[idx];
        }
    }

    fn is_degenerate(&self, area_eps: f64) -> bool {
        self.pl.vertex_count() < 2 || self.area().abs() < area_eps
    }
}

/// Modellgröße einer Region (größte AABB-Kante) für relative Toleranzen.
fn region_scale(region: &[Component]) -> f64 {
    let mut s = 0.0f64;
    for c in region {
        for tp in std::iter::once(&c.outer).chain(c.holes.iter()) {
            if let Some(e) = tp.pl.extents() {
                s = s.max(e.max_x - e.min_x).max(e.max_y - e.min_y);
            }
        }
    }
    s
}

fn collapsed_area_eps(scale: f64) -> f64 {
    1e-8 * (1.0 + scale * scale)
}

// ─── Sketch2d-Baum → Region ──────────────────────────────────────────────

/// Löst den Profilbaum in eine Region auf. `base` läuft in derselben
/// In-Order-DFS wie [`Sketch2d::edge_count`], damit die Segment-Tags die
/// globalen Boundary-Gruppen-Ids des Originalbaums sind.
fn resolve_region(sketch: &Sketch2d, base: &mut u32) -> Option<Region> {
    match sketch {
        Sketch2d::Empty => Some(Vec::new()),
        Sketch2d::Prim(p) => {
            let tagged = prim_to_loop(p, *base)?;
            *base += p.edge_count();
            match tagged {
                None => Some(Vec::new()),
                Some(tp) => Some(vec![Component { outer: tp, holes: Vec::new() }]),
            }
        }
        Sketch2d::Union(a, b) => {
            let ra = resolve_region(a, base)?;
            let rb = resolve_region(b, base)?;
            union_regions(ra, rb)
        }
        Sketch2d::Difference(a, b) => {
            let ra = resolve_region(a, base)?;
            let rb = resolve_region(b, base)?;
            difference_regions(ra, &rb)
        }
        Sketch2d::Intersection(a, b) => {
            let ra = resolve_region(a, base)?;
            let rb = resolve_region(b, base)?;
            intersect_regions(&ra, &rb)
        }
        Sketch2d::Offset { inner, distance } => {
            let r = resolve_region(inner, base)?;
            if !distance.is_finite() {
                return None;
            }
            if *distance == 0.0 {
                return Some(r);
            }
            // Sketch2d-Konvention: positiv = wachsen. cavalier-Konvention:
            // positiv = links der Laufrichtung = innen bei CCW = Erosion.
            offset_region(&r, -(*distance as f64))
        }
    }
}

/// Ein Primitive als getaggte CCW-Loop. `Ok(None)` (= `Some(None)`) heißt
/// „leer, überspringen"; `None` heißt verweigern.
#[allow(clippy::type_complexity)]
fn prim_to_loop(p: &Sketch2dPrimitive, base: u32) -> Option<Option<TaggedPline>> {
    let mut tp = TaggedPline::new();
    match p {
        Sketch2dPrimitive::Circle { center, radius } => {
            if !(*radius > 0.0) {
                return Some(None);
            }
            let (cx, cy, r) = (center[0] as f64, center[1] as f64, *radius as f64);
            tp.push(cx + r, cy, 1.0, base);
            tp.push(cx - r, cy, 1.0, base);
        }
        Sketch2dPrimitive::Rectangle { center, half_extents } => {
            let (cx, cy) = (center[0] as f64, center[1] as f64);
            let (hx, hy) = (half_extents[0] as f64, half_extents[1] as f64);
            if !(hx > 0.0 && hy > 0.0) {
                return Some(None);
            }
            // Gruppenordnung von `Rectangle::sdf_with_edge`: 0=+x, 1=−x,
            // 2=+y, 3=−y (identisch zu `revolve_prim_boundary`).
            tp.push(cx - hx, cy - hy, 0.0, base + 3);
            tp.push(cx + hx, cy - hy, 0.0, base);
            tp.push(cx + hx, cy + hy, 0.0, base + 2);
            tp.push(cx - hx, cy + hy, 0.0, base + 1);
        }
        Sketch2dPrimitive::Polygon { vertices } => {
            if vertices.len() < 3 {
                return Some(None);
            }
            // Kanten-Id = Startvertex-Index (`sdf_polygon_with_edge`).
            for (i, v) in vertices.iter().enumerate() {
                if !v[0].is_finite() || !v[1].is_finite() {
                    return None;
                }
                tp.push(v[0] as f64, v[1] as f64, 0.0, base + i as u32);
            }
        }
        Sketch2dPrimitive::Sector { center, radius, start_angle, sweep_angle } => {
            let r = *radius as f64;
            let sw = *sweep_angle as f64;
            if !(r > 0.0) || sw.abs() < 1e-9 {
                return Some(None);
            }
            if sw.abs() > std::f64::consts::TAU - 1e-3 {
                return None; // Voll-Disc mit Schlitz: kein sauberer Loop
            }
            let (cx, cy) = (center[0] as f64, center[1] as f64);
            let a0 = *start_angle as f64;
            let p0 = [cx + r * a0.cos(), cy + r * a0.sin()];
            let a1 = a0 + sw;
            let p1 = [cx + r * a1.cos(), cy + r * a1.sin()];
            // Gruppen wie `revolve_prim_boundary`: 0 = Radius zu p0,
            // 1 = Bogen, 2 = Radius zurück.
            tp.push(cx, cy, 0.0, base);
            push_arc(&mut tp, p0, r, a0, sw, base + 1);
            tp.push(p1[0], p1[1], 0.0, base + 2);
        }
        Sketch2dPrimitive::Path { segments, closed } => {
            if !*closed || segments.is_empty() {
                return None;
            }
            if segments
                .iter()
                .any(|s| matches!(s, SketchSegment::QuadBezier { .. }))
            {
                return None; // kein cavalier-Gegenstück; Biarc ist ein Folgeticket
            }
            // Kettenschluss prüfen: gestrippte Revolve-Profile (#390) haben
            // Lücken auf der Achse → verweigern (Achs-Gate greift ohnehin).
            let n = segments.len();
            for i in 0..n {
                let e = segments[i].end_point();
                let s = segments[(i + 1) % n].start_point();
                let gap = ((e[0] - s[0]).powi(2) + (e[1] - s[1]).powi(2)).sqrt();
                if gap > 1e-4 {
                    return None;
                }
            }
            let groups = g1_group_segments(segments, PATH_G1_COS_THRESHOLD);
            for (seg, g) in segments.iter().zip(groups.iter()) {
                let tag = base + *g;
                match seg {
                    SketchSegment::Line { start, end } => {
                        if !all_finite2(start) || !all_finite2(end) {
                            return None;
                        }
                        tp.push(start[0] as f64, start[1] as f64, 0.0, tag);
                    }
                    SketchSegment::Arc { center, radius, start_angle, sweep_angle } => {
                        if !all_finite2(center)
                            || !radius.is_finite()
                            || !start_angle.is_finite()
                            || !sweep_angle.is_finite()
                            || !(*radius > 0.0)
                        {
                            return None;
                        }
                        let (cx, cy, r) = (center[0] as f64, center[1] as f64, *radius as f64);
                        let a0 = *start_angle as f64;
                        let sw = *sweep_angle as f64;
                        let p0 = [cx + r * a0.cos(), cy + r * a0.sin()];
                        push_arc(&mut tp, p0, r, a0, sw, tag);
                        // Endpunkt setzt das nächste Segment (Kette geprüft).
                    }
                    SketchSegment::QuadBezier { .. } => unreachable!("oben verweigert"),
                }
            }
        }
    }
    tp.seal();
    let scale = tp
        .pl
        .extents()
        .map(|e| (e.max_x - e.min_x).max(e.max_y - e.min_y))
        .unwrap_or(0.0);
    if tp.is_degenerate(collapsed_area_eps(scale)) {
        return Some(None);
    }
    tp.ensure_ccw();
    if tp.pl.scan_for_self_intersect() {
        return None; // Boolean-/Offset-Vorbedingung (Upstream #72/#38)
    }
    Some(Some(tp))
}

fn all_finite2(p: &[f32; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

/// Bogen als Bulge-Vertex(e) anfügen; Bögen > 3π/2 werden halbiert
/// (Bulge = tan(sweep/4) explodiert bei sweep → 2π), Tags dupliziert.
fn push_arc(tp: &mut TaggedPline, start: [f64; 2], r: f64, a0: f64, sweep: f64, tag: u32) {
    if sweep.abs() > 1.5 * std::f64::consts::PI {
        let half = sweep * 0.5;
        let mid_a = a0 + half;
        // Startpunkt-Zentrum aus Startpunkt + Winkel rekonstruieren.
        let c = [start[0] - r * a0.cos(), start[1] - r * a0.sin()];
        let mid = [c[0] + r * mid_a.cos(), c[1] + r * mid_a.sin()];
        push_arc(tp, start, r, a0, half, tag);
        push_arc(tp, mid, r, mid_a, half, tag);
        return;
    }
    tp.push(start[0], start[1], (sweep * 0.25).tan(), tag);
}

// ─── Paar-Booleans mit Provenance-Restitch ───────────────────────────────

/// Lagebeziehung zweier geschlossener CCW-Loops.
fn relation(a: &TaggedPline, b: &TaggedPline) -> PlineContainsResult {
    a.pl.contains(&b.pl)
}

/// Boolean zweier Loops mit Provenance. Erwartet den `Intersected`-Fall;
/// alle degenerierten Fälle behandeln die Aufrufer über [`relation`].
/// Overlapping (geometrisch identische Pfade) wird je Op kanonisch
/// beantwortet. Rückgabe `(pos, neg)`; `None` = verweigern.
#[allow(clippy::type_complexity)]
fn bool_pair(
    a: &TaggedPline,
    b: &TaggedPline,
    op: BooleanOp,
) -> Option<(Vec<TaggedPline>, Vec<TaggedPline>)> {
    let scale = {
        let ea = a.pl.extents()?;
        let eb = b.pl.extents()?;
        (ea.max_x - ea.min_x)
            .max(ea.max_y - ea.min_y)
            .max(eb.max_x - eb.min_x)
            .max(eb.max_y - eb.min_y)
    };
    let area_eps = collapsed_area_eps(scale);
    let res = a.pl.boolean(&b.pl, op);
    match res.result_info {
        BooleanResultInfo::Intersected => {}
        BooleanResultInfo::Overlapping => {
            // Identischer Pfad: kanonische Antworten.
            return Some(match op {
                BooleanOp::Or | BooleanOp::And => (vec![a.clone()], Vec::new()),
                BooleanOp::Not | BooleanOp::Xor => (Vec::new(), Vec::new()),
            });
        }
        // Nicht-geschnittene Fälle sollen die Aufrufer über `relation`
        // abfangen; hier hieße es, die Tag-Zuordnung zu raten → verweigern.
        _ => return None,
    }
    let mut pos = Vec::new();
    for rp in &res.pos_plines {
        if let Some(tp) = restitch(rp.subslices.as_slice(), a, b, area_eps)? {
            pos.push(tp);
        }
    }
    let mut neg = Vec::new();
    for rp in &res.neg_plines {
        if let Some(tp) = restitch(rp.subslices.as_slice(), a, b, area_eps)? {
            neg.push(tp);
        }
    }
    Some((pos, neg))
}

/// Baut eine Ergebnis-Loop aus ihren `subslices` selbst zusammen, damit
/// jedes Segment seine Quellsegment-Provenance trägt (Audit Punkt 4:
/// `source_is_pline1` + Vertex-Index-Fenster). `Ok(None)` = kollabierte
/// Null-Flächen-Loop (weglassen).
fn restitch(
    slices: &[BooleanPlineSlice<f64>],
    a: &TaggedPline,
    b: &TaggedPline,
    area_eps: f64,
) -> Option<Option<TaggedPline>> {
    if slices.is_empty() {
        return None;
    }
    let mut tp = TaggedPline::new();
    for slice in slices {
        let (src_pl, src_tags) = if slice.source_is_pline1 {
            (&a.pl, &a.src)
        } else {
            (&b.pl, &b.src)
        };
        let n_src = src_pl.vertex_count();
        let vd = &slice.view_data;
        let view = vd.view(src_pl);
        let m = view.vertex_count();
        // View-Segment k → Quellsegment-Index (Herleitung aus
        // `PlineViewData::get_vertex`): vorwärts `start_index + k`,
        // invertiert `start_index + end_index_offset − k` (wrapping).
        for k in 0..m.saturating_sub(1) {
            let v = view.at(k);
            let src_seg = if vd.inverted_direction {
                (vd.start_index + vd.end_index_offset - k) % n_src
            } else {
                (vd.start_index + k) % n_src
            };
            tp.push(v.x, v.y, v.bulge, src_tags[src_seg]);
        }
    }
    tp.seal();
    if tp.is_degenerate(area_eps) {
        return Some(None);
    }
    tp.ensure_ccw();
    Some(Some(tp))
}

/// `a ∖ b` zweier Loops: `(pos, neg)`; `neg` = b liegt strikt innen (Loch).
#[allow(clippy::type_complexity)]
fn pline_not(a: &TaggedPline, b: &TaggedPline) -> Option<(Vec<TaggedPline>, Vec<TaggedPline>)> {
    match relation(a, b) {
        PlineContainsResult::Disjoint => Some((vec![a.clone()], Vec::new())),
        PlineContainsResult::Pline1InsidePline2 => Some((Vec::new(), Vec::new())),
        PlineContainsResult::Pline2InsidePline1 => {
            Some((vec![a.clone()], vec![b.clone()]))
        }
        PlineContainsResult::Intersected => bool_pair(a, b, BooleanOp::Not),
        PlineContainsResult::InvalidInput => None,
    }
}

/// `a ∩ b` zweier Loops (nur pos; verschachtelte Neg-Ergebnisse → verweigern).
fn pline_and(a: &TaggedPline, b: &TaggedPline) -> Option<Vec<TaggedPline>> {
    match relation(a, b) {
        PlineContainsResult::Disjoint => Some(Vec::new()),
        PlineContainsResult::Pline1InsidePline2 => Some(vec![a.clone()]),
        PlineContainsResult::Pline2InsidePline1 => Some(vec![b.clone()]),
        PlineContainsResult::Intersected => {
            let (pos, neg) = bool_pair(a, b, BooleanOp::And)?;
            if !neg.is_empty() {
                return None; // eingeschlossene Tasche: Tiefe > 2
            }
            Some(pos)
        }
        PlineContainsResult::InvalidInput => None,
    }
}

/// `a ∪ b` zweier ÜBERLAPPENDER Loops: genau eine pos-Loop (+ Taschen).
#[allow(clippy::type_complexity)]
fn pline_or(a: &TaggedPline, b: &TaggedPline) -> Option<(TaggedPline, Vec<TaggedPline>)> {
    let (pos, neg) = bool_pair(a, b, BooleanOp::Or)?;
    if pos.len() != 1 {
        return None;
    }
    Some((pos.into_iter().next().unwrap(), neg))
}

fn point_inside(pl: &Polyline<f64>, p: [f64; 2]) -> bool {
    pl.winding_number(cavalier_contours::core::math::Vector2::new(p[0], p[1])) != 0
}

fn first_point(tp: &TaggedPline) -> [f64; 2] {
    let v = tp.pl.at(0);
    [v.x, v.y]
}

// ─── Region-Algebra (loop-paarweise, pos/neg-Buchführung) ────────────────

fn union_regions(mut acc: Region, other: Region) -> Option<Region> {
    acc.extend(other);
    'merge: loop {
        for i in 0..acc.len() {
            for j in (i + 1)..acc.len() {
                match relation(&acc[i].outer, &acc[j].outer) {
                    PlineContainsResult::Disjoint => continue,
                    PlineContainsResult::InvalidInput => return None,
                    _ => {
                        let cj = acc.remove(j);
                        let ci = acc.remove(i);
                        let merged = union_components(ci, cj)?;
                        acc.push(merged);
                        continue 'merge;
                    }
                }
            }
        }
        return Some(acc);
    }
}

/// Vereinigung zweier INTERAGIERENDER Komponenten (Outer schneiden sich
/// oder enthalten einander). Loch-Formel: `H(a∪b) = (H_a ∖ B) ∪ (H_b ∖
/// outer_a)` — der Term `H_a ∩ H_b` steckt in `H_a ∖ B` (Subtraktion des
/// vollen Materials von B) und darf von der b-Seite nicht doppelt kommen.
fn union_components(c1: Component, c2: Component) -> Option<Component> {
    let (outer, pockets) = match relation(&c1.outer, &c2.outer) {
        PlineContainsResult::Intersected => {
            let (o, p) = pline_or(&c1.outer, &c2.outer)?;
            (o, p)
        }
        PlineContainsResult::Pline1InsidePline2 => {
            // outer1 ⊂ outer2: liegt outer1 ganz in einem Loch von c2,
            // wäre das eine Insel im Loch (Tiefe 3) → verweigern.
            if c2
                .holes
                .iter()
                .any(|h| matches!(relation(h, &c1.outer), PlineContainsResult::Pline2InsidePline1))
            {
                return None;
            }
            (c2.outer.clone(), Vec::new())
        }
        PlineContainsResult::Pline2InsidePline1 => {
            if c1
                .holes
                .iter()
                .any(|h| matches!(relation(h, &c2.outer), PlineContainsResult::Pline2InsidePline1))
            {
                return None;
            }
            (c1.outer.clone(), Vec::new())
        }
        _ => return None,
    };
    let mut holes = pockets;
    // H_a ∖ Material(B): (h ∖ outer_b) ∪ (h ∩ h_b) — beides paarweise.
    for h in &c1.holes {
        holes.extend(hole_minus_component(h, &c2)?);
    }
    // H_b ∖ outer_a (der ∩-Term kam schon von der a-Seite).
    for h in &c2.holes {
        let (pos, neg) = pline_not(h, &c1.outer)?;
        if !neg.is_empty() {
            return None; // outer_a-Insel im Loch: Tiefe 3
        }
        holes.extend(pos);
    }
    Some(Component { outer, holes })
}

/// `h ∖ Material(c)` für eine Loch-Loop `h`: `(h ∖ outer_c) ∪ (h ∩ h_c)`.
fn hole_minus_component(h: &TaggedPline, c: &Component) -> Option<Vec<TaggedPline>> {
    let mut out = Vec::new();
    let (pos, neg) = pline_not(h, &c.outer)?;
    if !neg.is_empty() {
        return None; // outer_c läge als Insel im Loch: Tiefe 3
    }
    out.extend(pos);
    for hc in &c.holes {
        out.extend(pline_and(h, hc)?);
    }
    Some(out)
}

fn intersect_regions(a: &Region, b: &Region) -> Option<Region> {
    let mut out = Vec::new();
    for ca in a {
        for cb in b {
            out.extend(intersect_components(ca, cb)?);
        }
    }
    Some(out)
}

fn intersect_components(a: &Component, b: &Component) -> Option<Vec<Component>> {
    let pieces = pline_and(&a.outer, &b.outer)?;
    let mut comps: Vec<Component> = pieces
        .into_iter()
        .map(|outer| Component { outer, holes: Vec::new() })
        .collect();
    for h in a.holes.iter().chain(b.holes.iter()) {
        let mut next = Vec::new();
        for c in comps {
            next.extend(subtract_loop(c, h)?);
        }
        comps = next;
    }
    Some(comps)
}

fn difference_regions(a: Region, b: &Region) -> Option<Region> {
    let mut acc = a;
    for cb in b {
        let mut next = Vec::new();
        for ca in acc {
            next.extend(subtract_component(ca, cb)?);
        }
        acc = next;
    }
    Some(acc)
}

/// `c ∖ Material(b)`: erst `c ∖ outer_b`, dann Material von `c` innerhalb
/// der Löcher von `b` wieder hinzu (disjunkt zu ersterem).
fn subtract_component(c: Component, b: &Component) -> Option<Vec<Component>> {
    let mut out = subtract_loop(c.clone(), &b.outer)?;
    for hb in &b.holes {
        let hb_comp = Component { outer: hb.clone(), holes: Vec::new() };
        out.extend(intersect_components(&c, &hb_comp)?);
    }
    Some(out)
}

/// `c ∖ l` für eine einzelne Loop `l`: überlappende Löcher werden zuerst
/// in `l` einfusioniert, dann EIN `Not` gegen die Außenkontur; alte Löcher
/// werden den Ergebnis-Stücken per Containment zugeteilt.
fn subtract_loop(c: Component, l: &TaggedPline) -> Option<Vec<Component>> {
    let mut merged = l.clone();
    let rest: Vec<TaggedPline>;
    let mut pending = c.holes;
    'outer: loop {
        for (i, h) in pending.iter().enumerate() {
            match relation(&merged, h) {
                PlineContainsResult::Disjoint => continue,
                PlineContainsResult::Pline1InsidePline2 => {
                    // merged ⊂ h: das Loch deckt den Abzug bereits ab.
                    merged = h.clone();
                    pending.remove(i);
                    continue 'outer;
                }
                PlineContainsResult::Pline2InsidePline1 => {
                    pending.remove(i);
                    continue 'outer;
                }
                PlineContainsResult::Intersected => {
                    let h = pending.remove(i);
                    let (m, pockets) = pline_or(&merged, &h)?;
                    if !pockets.is_empty() {
                        return None; // Insel zwischen Loch und Abzug: Tiefe 3
                    }
                    merged = m;
                    continue 'outer;
                }
                PlineContainsResult::InvalidInput => return None,
            }
        }
        rest = pending;
        break;
    }
    let (pieces, cut_holes) = pline_not(&c.outer, &merged)?;
    let mut comps: Vec<Component> = pieces
        .into_iter()
        .map(|outer| Component { outer, holes: Vec::new() })
        .collect();
    for h in rest.into_iter().chain(cut_holes) {
        let p = first_point(&h);
        if let Some(target) = comps.iter_mut().find(|comp| point_inside(&comp.outer.pl, p)) {
            target.holes.push(h);
        }
        // Loch in keinem Stück: lag im weggeschnittenen Bereich → entfällt.
    }
    Some(comps)
}

// ─── Shape-Offset (Erosion / Sketch2d::Offset) mit Rückprojektion ────────

/// Offsettet eine Region um `delta` in cavalier-Konvention (positiv =
/// innen bei CCW = Erosion; negativ = Dilation). Provenance per exakter
/// Rückprojektion: jedes Ergebnis-Segment liegt konstruktiv bei Abstand
/// |delta| zu genau einem Quellsegment; zwei gleiche Minima = Join-Bogen
/// am gemeinsamen Quell-Vertex → erbt vom im Umlaufsinn vorangehenden
/// Segment.
fn offset_region(region: &Region, delta: f64) -> Option<Region> {
    if region.is_empty() {
        return Some(Vec::new());
    }
    let scale = region_scale(region);
    let area_eps = collapsed_area_eps(scale);
    // Quellsegmente (alle Loops, Original-Orientierung) für die
    // Rückprojektion einsammeln.
    let mut sources: Vec<(SegGeom, u32)> = Vec::new();
    let mut shape_plines: Vec<Polyline<f64>> = Vec::new();
    for c in region {
        for (is_hole, tp) in std::iter::once((false, &c.outer))
            .chain(c.holes.iter().map(|h| (true, h)))
        {
            collect_seg_geoms(tp, &mut sources);
            let mut pl = tp.pl.clone();
            if is_hole {
                pl.invert_direction_mut(); // Shape-Konvention: Löcher CW
            }
            shape_plines.push(pl);
        }
    }
    let shape = Shape::from_plines(shape_plines);
    let result = shape.parallel_offset(delta, ShapeOffsetOptions::default());

    let a = delta.abs();
    let on_tol = 1e-6 * (1.0 + scale + a);
    let tie_eps = 1e-9 * (1.0 + scale + a);

    let mut outers: Vec<TaggedPline> = Vec::new();
    let mut holes: Vec<TaggedPline> = Vec::new();
    for (is_hole, ip) in result
        .ccw_plines
        .iter()
        .map(|p| (false, p))
        .chain(result.cw_plines.iter().map(|p| (true, p)))
    {
        let tp = backproject_tags(&ip.polyline, &sources, a, on_tol, tie_eps, area_eps)?;
        let Some(mut tp) = tp else { continue };
        tp.ensure_ccw();
        if is_hole {
            holes.push(tp);
        } else {
            outers.push(tp);
        }
    }
    // Nesting neu aufbauen: jedes Loch gehört in genau eine Außenkontur;
    // ineinanderliegende Außenkonturen wären Tiefe > 2 → verweigern.
    for i in 0..outers.len() {
        for j in 0..outers.len() {
            if i != j && point_inside(&outers[j].pl, first_point(&outers[i])) {
                return None;
            }
        }
    }
    let mut comps: Vec<Component> = outers
        .into_iter()
        .map(|outer| Component { outer, holes: Vec::new() })
        .collect();
    'holes: for h in holes {
        let p = first_point(&h);
        for c in comps.iter_mut() {
            if point_inside(&c.outer.pl, p) {
                c.holes.push(h);
                continue 'holes;
            }
        }
        return None; // Loch ohne Außenkontur: unerwartet
    }
    Some(comps)
}

/// Exakte Segment-Geometrie (Linie oder Bogen) für die Rückprojektion.
enum SegGeom {
    Line { p1: [f64; 2], p2: [f64; 2] },
    Arc { c: [f64; 2], r: f64, a1: f64, sweep: f64, p1: [f64; 2], p2: [f64; 2] },
}

fn collect_seg_geoms(tp: &TaggedPline, out: &mut Vec<(SegGeom, u32)>) {
    let n = tp.pl.vertex_count();
    for i in 0..n {
        let v1 = tp.pl.at(i);
        let v2 = tp.pl.at((i + 1) % n);
        let p1 = [v1.x, v1.y];
        let p2 = [v2.x, v2.y];
        let geom = if v1.bulge.abs() < 1e-14 {
            SegGeom::Line { p1, p2 }
        } else {
            let (c, r, a1, sweep) = bulge_arc(p1, p2, v1.bulge);
            SegGeom::Arc { c, r, a1, sweep, p1, p2 }
        };
        out.push((geom, tp.src[i]));
    }
}

/// Rekonstruiert Zentrum/Radius/Startwinkel/Sweep eines Bulge-Segments.
fn bulge_arc(p1: [f64; 2], p2: [f64; 2], b: f64) -> ([f64; 2], f64, f64, f64) {
    let theta = 4.0 * b.atan();
    let dx = p2[0] - p1[0];
    let dy = p2[1] - p1[1];
    let chord = (dx * dx + dy * dy).sqrt();
    let s = 0.5 * chord;
    let dir = [dx / chord, dy / chord];
    let m = [0.5 * (p1[0] + p2[0]), 0.5 * (p1[1] + p2[1])];
    let dcm = s / (0.5 * theta).tan();
    let center = [m[0] - dir[1] * dcm, m[1] + dir[0] * dcm];
    let radius = (s / (0.5 * theta).sin()).abs();
    let a1 = (p1[1] - center[1]).atan2(p1[0] - center[0]);
    (center, radius, a1, theta)
}

fn wrap_pos(a: f64) -> f64 {
    let mut a = a % std::f64::consts::TAU;
    if a < 0.0 {
        a += std::f64::consts::TAU;
    }
    a
}

fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn seg_geom_dist(g: &SegGeom, p: [f64; 2]) -> f64 {
    match g {
        SegGeom::Line { p1, p2 } => {
            let dx = p2[0] - p1[0];
            let dy = p2[1] - p1[1];
            let len2 = dx * dx + dy * dy;
            if len2 < 1e-24 {
                return dist2(p, *p1);
            }
            let t = (((p[0] - p1[0]) * dx + (p[1] - p1[1]) * dy) / len2).clamp(0.0, 1.0);
            dist2(p, [p1[0] + t * dx, p1[1] + t * dy])
        }
        SegGeom::Arc { c, r, a1, sweep, p1, p2 } => {
            let ap = (p[1] - c[1]).atan2(p[0] - c[0]);
            let inside = if *sweep > 0.0 {
                wrap_pos(ap - a1) <= *sweep
            } else {
                wrap_pos(a1 - ap) <= -sweep
            };
            if inside {
                (dist2(p, *c) - r).abs()
            } else {
                dist2(p, *p1).min(dist2(p, *p2))
            }
        }
    }
}

/// Mittelpunkt eines Ergebnis-Segments (Linie oder Bogen).
fn pline_seg_mid(pl: &Polyline<f64>, i: usize) -> [f64; 2] {
    let n = pl.vertex_count();
    let v1 = pl.at(i);
    let v2 = pl.at((i + 1) % n);
    let p1 = [v1.x, v1.y];
    let p2 = [v2.x, v2.y];
    if v1.bulge.abs() < 1e-14 {
        [0.5 * (p1[0] + p2[0]), 0.5 * (p1[1] + p2[1])]
    } else {
        let (c, r, a1, sweep) = bulge_arc(p1, p2, v1.bulge);
        let a = a1 + 0.5 * sweep;
        [c[0] + r * a.cos(), c[1] + r * a.sin()]
    }
}

/// Tagt eine Offset-Ergebnis-Loop per Rückprojektion gegen die
/// Quellsegmente. `Ok(None)` = kollabierte Loop (weglassen).
fn backproject_tags(
    pl: &Polyline<f64>,
    sources: &[(SegGeom, u32)],
    a: f64,
    on_tol: f64,
    tie_eps: f64,
    area_eps: f64,
) -> Option<Option<TaggedPline>> {
    let n = pl.vertex_count();
    if n < 2 || pl.area().abs() < area_eps {
        return Some(None);
    }
    let mut tags = vec![JOIN; n];
    for (i, tag) in tags.iter_mut().enumerate() {
        let mid = pline_seg_mid(pl, i);
        let mut dmin = f64::MAX;
        let mut best = JOIN;
        for (g, t) in sources {
            let d = seg_geom_dist(g, mid);
            if d < dmin - tie_eps {
                dmin = d;
                best = *t;
            } else if d < dmin + tie_eps {
                if *t != best {
                    best = JOIN; // echter Tie zweier Quellen → Join-Bogen
                }
                dmin = dmin.min(d);
            }
        }
        if (dmin - a).abs() > on_tol {
            return None; // Ergebnis-Segment liegt nicht auf der Offset-Kurve
        }
        *tag = best;
    }
    let mut tp = TaggedPline::new();
    for i in 0..n {
        let v = pl.at(i);
        tp.push(v.x, v.y, v.bulge, tags[i]);
    }
    tp.seal();
    if tp.is_degenerate(area_eps) {
        return Some(None);
    }
    Some(Some(tp))
}

// ─── Region → Sketch2d + wall_subs-Tabelle ───────────────────────────────

/// Baut aus der erodierten Region den finalen Profilbaum plus die
/// propagierte `wall_subs`-Tabelle (Audit Punkt 4). `None` = verweigern
/// (Reserved-Range-Kollision, unattributierbare Gruppe).
///
/// Doppelte Subs sind hier LEGITIM: sie entstehen genau dann, wenn ein
/// Boolean eine Quellwand in mehrere kollineare/kozirkulare Stücke
/// spaltet — dieselbe Semantik wie eine durch einen Slot gespaltene
/// Box-Fläche heute (beide Patches teilen den Branch, der Carrier ist
/// identisch). Join-Bögen bekommen dagegen eigene deterministische Subs
/// aus [`JOIN_ARC_SUB_BASE`], weil sie neue Wände mit EIGENEM Carrier
/// sind.
fn region_to_profile(
    region: &Region,
    old_ws: Option<&[u32]>,
) -> Option<(Sketch2d, Vec<u32>)> {
    let mut table: Vec<u32> = Vec::new();
    let mut next_join_sub = JOIN_ARC_SUB_BASE;
    let mut tree: Option<Sketch2d> = None;
    for c in region {
        let outer = loop_to_path(&c.outer, old_ws, &mut table, &mut next_join_sub)?;
        let mut node = Sketch2d::Prim(outer);
        if !c.holes.is_empty() {
            let mut holes_node: Option<Sketch2d> = None;
            for h in &c.holes {
                let hp =
                    Sketch2d::Prim(loop_to_path(h, old_ws, &mut table, &mut next_join_sub)?);
                holes_node = Some(match holes_node {
                    None => hp,
                    Some(prev) => Sketch2d::Union(Box::new(prev), Box::new(hp)),
                });
            }
            node = Sketch2d::Difference(Box::new(node), Box::new(holes_node.unwrap()));
        }
        tree = Some(match tree {
            None => node,
            Some(prev) => Sketch2d::Union(Box::new(prev), Box::new(node)),
        });
    }
    let tree = tree?;
    // Vertrag: Tabelle deckt exakt die Boundary-Gruppen des neuen Baums.
    debug_assert_eq!(table.len() as u32, tree.edge_count());
    // Reserved-Range-Wächter: das Join-Band darf nicht in die Cap-Subs
    // laufen (Original-Subs ≥ JOIN_ARC_SUB_BASE verweigert `loop_to_path`
    // bereits beim Mappen).
    if next_join_sub >= BranchId::EXTRUDE_CAP_BOTTOM_SUB {
        return None;
    }
    Some((tree, table))
}

/// Eine getaggte Loop als geschlossener `Path`; hängt je G1-Gruppe die
/// (durch `old_ws` übersetzte) stabile Sub-Id an `table` an. Gruppen ohne
/// jede Quellwand (reine Join-Bögen) bekommen die nächste Sub aus dem
/// Join-Band.
fn loop_to_path(
    tp: &TaggedPline,
    old_ws: Option<&[u32]>,
    table: &mut Vec<u32>,
    next_join_sub: &mut u32,
) -> Option<Sketch2dPrimitive> {
    let n = tp.pl.vertex_count();
    if n < 2 {
        return None;
    }
    let mut segs: Vec<SketchSegment> = Vec::with_capacity(n);
    let mut tags: Vec<u32> = Vec::with_capacity(n);
    for i in 0..n {
        let v1 = tp.pl.at(i);
        let v2 = tp.pl.at((i + 1) % n);
        let p1 = [v1.x as f32, v1.y as f32];
        let p2 = [v2.x as f32, v2.y as f32];
        if v1.bulge.abs() < 1e-12 {
            segs.push(SketchSegment::Line { start: p1, end: p2 });
        } else {
            let (c, r, a1, sweep) = bulge_arc([v1.x, v1.y], [v2.x, v2.y], v1.bulge);
            segs.push(SketchSegment::Arc {
                center: [c[0] as f32, c[1] as f32],
                radius: r as f32,
                start_angle: a1 as f32,
                sweep_angle: sweep as f32,
            });
        }
        tags.push(tp.src[i]);
    }
    // Kozirkulare Runs verschmelzen: cavalier sät Kreis-Plines an festen
    // Punkten (c ± r), sodass ein durchgehender Kreisbogen als ZWEI
    // Bulge-Segmente mit einer künstlichen Naht zurückkommt — typisch
    // exakt auf einer Symmetriezeile (y = Kreiszentrum). Die Naht ist
    // nicht nur kosmetisch: die Even-Odd-Parität von `path_is_inside`
    // verlangt bitgleiche Nahtpunkt-Werte benachbarter Segmente, Arcs
    // leiten ihre Endpunkte aber aus Winkel+Zentrum her (±1e-16-Rauschen)
    // — eine Naht auf einer Gitterzeile kippt dann die Parität der
    // ganzen Zeile (gemessen: MDC-χ-Assert auf dem erodierten
    // Union-Profil). Ein Segment je Kreisbogen hat keine Naht.
    merge_cocircular_runs(&mut segs, &mut tags);
    let n = segs.len();
    // Loop so rotieren, dass sie an einem G1-Bruch beginnt — sonst zählt
    // die lineare Gruppierung (`g1_group_segments`, ohne Wrap-Merge) eine
    // über die Naht laufende glatte Wand doppelt. Deterministische Wahl:
    // der Bruch, dessen Folgesegment das kleinste Tag trägt (JOIN sortiert
    // per u32::MAX nach hinten).
    let is_break = |prev: &SketchSegment, cur: &SketchSegment| -> bool {
        !(tangent_at_joint(prev, cur, PATH_G1_COS_THRESHOLD) && same_carrier_like(prev, cur))
    };
    let breaks: Vec<usize> = (0..n)
        .filter(|&i| is_break(&segs[(i + n - 1) % n], &segs[i]))
        .collect();
    if let Some(&start) = breaks
        .iter()
        .min_by_key(|&&i| (tags[i], i))
    {
        segs.rotate_left(start);
        tags.rotate_left(start);
    }
    // Je G1-Gruppe die kleinste Quell-Sub (Determinismus-Regel wie
    // `traced_group_ids`) — durch die alte Tabelle übersetzt. Gruppen ohne
    // Quellwand (reine Join-Bögen) ziehen die nächste Band-Sub.
    let groups = g1_group_segments(&segs, PATH_G1_COS_THRESHOLD);
    let group_count = groups.iter().copied().max().map(|g| g + 1).unwrap_or(0) as usize;
    let mut per_group: Vec<Option<u32>> = vec![None; group_count];
    for (g, tag) in groups.iter().zip(tags.iter()) {
        if *tag == JOIN {
            continue;
        }
        let mapped = match old_ws {
            Some(t) => *t.get(*tag as usize)?,
            None => *tag,
        };
        if mapped >= JOIN_ARC_SUB_BASE {
            return None; // Original-Sub kollidiert mit dem Join-Band
        }
        let slot = &mut per_group[*g as usize];
        *slot = Some(match slot {
            Some(prev) => (*prev).min(mapped),
            None => mapped,
        });
    }
    for g in per_group {
        match g {
            Some(sub) => table.push(sub),
            None => {
                table.push(*next_join_sub);
                *next_join_sub += 1;
            }
        }
    }
    Some(Sketch2dPrimitive::Path { segments: segs, closed: true })
}

/// Verschmilzt zyklisch benachbarte Bogensegmente desselben Kreises mit
/// demselben Provenance-Tag zu EINEM Bogen (siehe Kommentar am Aufrufer:
/// beseitigt cavaliers künstliche Saat-Nähte, die die Even-Odd-Parität
/// auf Symmetriezeilen kippen können). Läuft bis zum Fixpunkt; ein voller
/// Kreis kollabiert zu einem einzelnen 2π-Bogen.
fn merge_cocircular_runs(segs: &mut Vec<SketchSegment>, tags: &mut Vec<u32>) {
    'again: loop {
        let n = segs.len();
        if n < 2 {
            return;
        }
        for i in 0..n {
            let j = (i + 1) % n;
            let merged = match (&segs[i], &segs[j]) {
                (
                    SketchSegment::Arc { center: ca, radius: ra, start_angle: sa, sweep_angle: wa },
                    SketchSegment::Arc { center: cb, radius: rb, sweep_angle: wb, .. },
                ) if tags[i] == tags[j] => {
                    let tol = (ra.abs() * 1e-5).max(1e-6);
                    let same_circle = (ca[0] - cb[0]).abs() <= tol
                        && (ca[1] - cb[1]).abs() <= tol
                        && (ra - rb).abs() <= tol;
                    let same_dir = (*wa >= 0.0) == (*wb >= 0.0);
                    let total = wa + wb;
                    if same_circle && same_dir && total.abs() <= std::f32::consts::TAU + 1e-4 {
                        Some(SketchSegment::Arc {
                            center: *ca,
                            radius: *ra,
                            start_angle: *sa,
                            sweep_angle: total.clamp(
                                -std::f32::consts::TAU,
                                std::f32::consts::TAU,
                            ),
                        })
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(m) = merged {
                segs[i] = m;
                segs.remove(j);
                tags.remove(j);
                continue 'again;
            }
        }
        return;
    }
}

/// Lokaler Spiegel von `fielddraft_protocol::sketch2d::same_carrier` (dort
/// privat): Linie↔Linie ja, Bogen↔Bogen bei gleichem Zentrum/Radius.
/// Zusammen mit [`tangent_at_joint`] ergibt das exakt das
/// `g1_continuous`-Prädikat, mit dem `g1_group_segments` gruppiert —
/// beide MÜSSEN synchron bleiben, sonst zählt die Rotations-Logik
/// Gruppen anders als die Laufzeit.
fn same_carrier_like(prev: &SketchSegment, cur: &SketchSegment) -> bool {
    match (prev, cur) {
        (SketchSegment::Line { .. }, SketchSegment::Line { .. }) => true,
        (SketchSegment::QuadBezier { .. }, SketchSegment::QuadBezier { .. }) => true,
        (
            SketchSegment::Arc { center: ca, radius: ra, .. },
            SketchSegment::Arc { center: cb, radius: rb, .. },
        ) => {
            let tol = (ra.abs() * 1e-5).max(1e-6);
            (ca[0] - cb[0]).abs() <= tol
                && (ca[1] - cb[1]).abs() <= tol
                && (ra - rb).abs() <= tol
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "offset_fold_tests.rs"]
mod tests;
