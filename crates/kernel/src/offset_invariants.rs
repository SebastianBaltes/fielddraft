//! offset_tree Pipeline-Invarianten (#384, Architektur-Review Follow-up 2).
//!
//! Die Offset/Shell-Normalisierung in `body::build_bounded_*` läuft als feste
//! Compiler-Pipeline auf dem CSG-Baum ([`crate::csg::CsgNode`]):
//!
//! 1. [`CsgNode::expand_shells`](crate::csg::CsgNode::expand_shells) — geschlossene
//!    Shells → `Difference(S, InnerWall(Offset(S, −t)))`; OFFENE Shells bleiben
//!    bewusst als `Shell`-Node (Feld-Pfad).
//! 2. [`CsgNode::push_down_offsets`](crate::csg::CsgNode::push_down_offsets) —
//!    Offsets durch die exakten Boolean-/Transform-Fälle an die Blätter schieben.
//! 3. [`CsgNode::fold_offset_leaves`](crate::csg::CsgNode::fold_offset_leaves) —
//!    Erosions-Offsets über Blättern analytisch einfalten.
//!
//! Dieses Modul macht die *Nachbedingungen* jeder Stufe als **pure,
//! read-only** Funktionen `check_*(&CsgNode) -> Result<(), String>`
//! greifbar. Verdrahtet werden sie über die `assert_after_*`-Helfer per
//! `debug_assert`-Semantik in ALLEN Baupfaden — in Release-Builds
//! (Produktions-WASM) faltet `cfg!(debug_assertions)` den ganzen Block weg,
//! die Feld-Auswertung ist also null-kostenneutral unberührt.
//!
//! **Kein Drift:** Die Entscheidungslogik ist NICHT kopiert. Invariante 2
//! ruft [`crate::csg::offset_can_push_through`] (dieselbe Funktion, die
//! `push_down_offset` selbst gatet); Invariante 3 negiert
//! [`crate::offset_fold::fold_would_accept`] (Wrapper um denselben
//! `try_fold_erosion`, den der Fold-Pass ausführt).

use crate::csg::{offset_can_push_through, CsgNode};

/// Kurzer Variantenname eines Knotens für Fehlermeldungen/Pfade.
fn kind(node: &CsgNode) -> &'static str {
    match node {
        CsgNode::Empty => "Empty",
        CsgNode::Leaf { .. } => "Leaf",
        CsgNode::Transform(..) => "Transform",
        CsgNode::Union(..) => "Union",
        CsgNode::Difference(..) => "Difference",
        CsgNode::Intersection(..) => "Intersection",
        CsgNode::SmoothUnion(..) => "SmoothUnion",
        CsgNode::SmoothDifference(..) => "SmoothDifference",
        CsgNode::Twist(..) => "Twist",
        CsgNode::Bend(..) => "Bend",
        CsgNode::Displace(..) => "Displace",
        CsgNode::Shell(..) => "Shell",
        CsgNode::InnerWall(..) => "InnerWall",
        CsgNode::Offset(..) => "Offset",
        CsgNode::Morph(..) => "Morph",
        CsgNode::MaskedBlend { .. } => "MaskedBlend",
        CsgNode::Extrude { .. } => "Extrude",
        CsgNode::Revolve { .. } => "Revolve",
        CsgNode::Helix { .. } => "Helix",
        CsgNode::Sweep { .. } => "Sweep",
        CsgNode::Loft { .. } => "Loft",
    }
}

/// Kind-Referenzen eines Knotens mit sprechendem Pfad-Label. Nur in den
/// (debug-only) Checkern benutzt, daher ist die Allokation belanglos.
fn children(node: &CsgNode) -> Vec<(&CsgNode, &'static str)> {
    match node {
        CsgNode::Transform(_, i) => vec![(i, "Transform")],
        CsgNode::Twist(i, _) => vec![(i, "Twist")],
        CsgNode::Bend(i, _) => vec![(i, "Bend")],
        CsgNode::Displace(i, _, _) => vec![(i, "Displace")],
        CsgNode::Shell(i, _, _) => vec![(i, "Shell")],
        CsgNode::InnerWall(i) => vec![(i, "InnerWall")],
        CsgNode::Offset(i, _) => vec![(i, "Offset")],
        CsgNode::Union(a, b) => vec![(a, "Union.l"), (b, "Union.r")],
        CsgNode::Difference(a, b) => vec![(a, "Difference.l"), (b, "Difference.r")],
        CsgNode::Intersection(a, b) => vec![(a, "Intersection.l"), (b, "Intersection.r")],
        CsgNode::SmoothUnion(a, b, _, _) => vec![(a, "SmoothUnion.l"), (b, "SmoothUnion.r")],
        CsgNode::SmoothDifference(a, b, _, _) => {
            vec![(a, "SmoothDifference.l"), (b, "SmoothDifference.r")]
        }
        CsgNode::Morph(a, b, _) => vec![(a, "Morph.l"), (b, "Morph.r")],
        CsgNode::MaskedBlend { a, b, .. } => vec![(a, "MaskedBlend.a"), (b, "MaskedBlend.b")],
        CsgNode::Empty
        | CsgNode::Leaf { .. }
        | CsgNode::Extrude { .. }
        | CsgNode::Revolve { .. }
        | CsgNode::Helix { .. }
        | CsgNode::Sweep { .. }
        | CsgNode::Loft { .. } => Vec::new(),
    }
}

// ─── Invariante 1: nach expand_shells kein geschlossener Shell ────────────

/// **Nach `expand_shells`:** kein *geschlossener* Shell-Node steht mehr im
/// Baum. Jeder verbleibende [`CsgNode::Shell`] muss offene Flächen tragen
/// (nicht-leerer `Vec<OpenSurface>` — der dokumentierte Feld-Pfad für offene
/// Shells); ein geschlossener (leerer Vec) hätte zu
/// `Difference(body, InnerWall(Offset(body, −t)))` expandiert werden müssen.
pub fn check_no_closed_shells(node: &CsgNode) -> Result<(), String> {
    check_no_closed_shells_at(node, "root")
}

fn check_no_closed_shells_at(node: &CsgNode, path: &str) -> Result<(), String> {
    if let CsgNode::Shell(_, thickness, open) = node {
        if open.is_empty() {
            return Err(format!(
                "{path}: geschlossener Shell-Node (thickness={thickness}) nach expand_shells — \
                 hätte zu Difference(body, InnerWall(Offset(body, −t))) expandiert werden müssen"
            ));
        }
    }
    for (child, label) in children(node) {
        check_no_closed_shells_at(child, &format!("{path}/{label}"))?;
    }
    Ok(())
}

// ─── Invariante 2: nach push_down_offsets keine pushbaren Offsets ─────────

/// **Nach `push_down_offsets`:** kein Offset-Node steht mehr über einem
/// Knoten, durch den [`crate::csg::offset_can_push_through`] ihn hätte schieben
/// können (pushbare Union bei δ>0, pushbare Intersection/Difference bei δ<0,
/// rigide Transform, weiterer Offset oder δ=0). Nutzt dieselbe
/// Entscheidungsfunktion wie der Pass — keine driftfähige Kopie.
pub fn check_offsets_pushed(node: &CsgNode) -> Result<(), String> {
    check_offsets_pushed_at(node, "root")
}

fn check_offsets_pushed_at(node: &CsgNode, path: &str) -> Result<(), String> {
    if let CsgNode::Offset(inner, delta) = node {
        if offset_can_push_through(inner, *delta) {
            return Err(format!(
                "{path}: Offset(δ={delta}) steht über einem {}, durch den push_down_offsets \
                 ihn hätte schieben können",
                kind(inner)
            ));
        }
    }
    for (child, label) in children(node) {
        check_offsets_pushed_at(child, &format!("{path}/{label}"))?;
    }
    Ok(())
}

// ─── Invariante 3: nach fold_offset_leaves nur echte Fold-Verweigerungen ──

/// **Nach `fold_offset_leaves`:** jedes verbleibende
/// `Offset(Leaf|Extrude|Revolve, δ)` mit Erosion (δ<0) ist eine ECHTE
/// dokumentierte Verweigerung — [`crate::offset_fold::fold_would_accept`]
/// (derselbe `try_fold_erosion`, den der Pass fährt) muss es ABLEHNEN. Würde
/// der Fold es akzeptieren (falten oder kollabieren), steht es fälschlich noch.
/// Dilation (δ>0) bleibt bewusst als `f − δ` stehen und ist keine Verletzung.
pub fn check_offset_leaves_folded(node: &CsgNode) -> Result<(), String> {
    check_offset_leaves_folded_at(node, "root")
}

fn check_offset_leaves_folded_at(node: &CsgNode, path: &str) -> Result<(), String> {
    if let CsgNode::Offset(inner, delta) = node {
        let foldable_leaf = matches!(
            **inner,
            CsgNode::Leaf { .. } | CsgNode::Extrude { .. } | CsgNode::Revolve { .. }
        );
        if foldable_leaf && *delta < 0.0 && crate::offset_fold::fold_would_accept(inner, -*delta) {
            return Err(format!(
                "{path}: Offset({}, δ={delta}) ist erosions-faltbar — fold_offset_leaves hätte es \
                 eingefaltet oder kollabiert, es steht aber noch als residualer f−δ",
                kind(inner)
            ));
        }
    }
    for (child, label) in children(node) {
        check_offset_leaves_folded_at(child, &format!("{path}/{label}"))?;
    }
    Ok(())
}

// ─── Invariante 4: InnerWall nur als rechtes Kind einer Difference ────────

/// **InnerWall-Positionsinvariante (ab `expand_shells` durchgehend):**
/// [`CsgNode::InnerWall`] kommt AUSSCHLIESSLICH als rechtes Kind einer
/// [`CsgNode::Difference`] vor. `collect_parity` in `csg.rs` (und die
/// Surface-Registry, `brep/weaving/surface_registry.rs`) setzen das hart
/// voraus. Gilt in jeder Pipelinestufe nach der Expansion.
pub fn check_inner_wall_position(node: &CsgNode) -> Result<(), String> {
    check_inner_wall_position_at(node, "root", false)
}

/// `inner_wall_legal_here` ist wahr, wenn `node` an einer Stelle steht, an der
/// ein InnerWall erlaubt ist (= das rechte Kind einer Difference).
fn check_inner_wall_position_at(
    node: &CsgNode,
    path: &str,
    inner_wall_legal_here: bool,
) -> Result<(), String> {
    if matches!(node, CsgNode::InnerWall(_)) && !inner_wall_legal_here {
        return Err(format!(
            "{path}: InnerWall-Node außerhalb der rechten Kind-Position einer Difference — \
             collect_parity setzt InnerWall AUSSCHLIESSLICH als Difference-Subtrahend voraus"
        ));
    }
    match node {
        CsgNode::Difference(a, b) => {
            // Linkes Kind: hier ist ein InnerWall verboten. Rechtes Kind: hier
            // ist genau EIN InnerWall die legale Position.
            check_inner_wall_position_at(a, &format!("{path}/Difference.l"), false)?;
            check_inner_wall_position_at(b, &format!("{path}/Difference.r"), true)?;
        }
        _ => {
            for (child, label) in children(node) {
                check_inner_wall_position_at(child, &format!("{path}/{label}"), false)?;
            }
        }
    }
    Ok(())
}

// ─── Verdrahtung: debug-only Pipeline-Gates ──────────────────────────────

/// Panik mit einheitlichem Präfix, wenn ein Checker eine Verletzung meldet.
#[inline]
fn assert_ok(result: Result<(), String>, pass: &str) {
    if let Err(e) = result {
        panic!("[offset_tree pipeline / {pass}] {e}");
    }
}

/// Debug-only Gate direkt NACH `expand_shells`: Invariante 1 + 4.
///
/// In Release-Builds ist `cfg!(debug_assertions)` konstant `false`; der Block
/// wird wegoptimiert, die Checker laufen nie — null Laufzeitkosten und keine
/// Rückwirkung auf die Feld-Auswertung.
#[inline]
pub fn assert_after_expand_shells(node: &CsgNode) {
    if cfg!(debug_assertions) {
        assert_ok(check_no_closed_shells(node), "expand_shells");
        assert_ok(check_inner_wall_position(node), "expand_shells");
    }
}

/// Debug-only Gate direkt NACH `push_down_offsets`: Invariante 2 + 4.
#[inline]
pub fn assert_after_push_down_offsets(node: &CsgNode) {
    if cfg!(debug_assertions) {
        assert_ok(check_offsets_pushed(node), "push_down_offsets");
        assert_ok(check_inner_wall_position(node), "push_down_offsets");
    }
}

/// Debug-only Gate direkt NACH `fold_offset_leaves`: Invariante 3 + 4.
#[inline]
pub fn assert_after_fold_offset_leaves(node: &CsgNode) {
    if cfg!(debug_assertions) {
        assert_ok(check_offset_leaves_folded(node), "fold_offset_leaves");
        assert_ok(check_inner_wall_position(node), "fold_offset_leaves");
    }
}

#[cfg(test)]
#[path = "offset_invariants_tests.rs"]
mod tests;
