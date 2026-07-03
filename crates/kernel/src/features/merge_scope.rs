//! Merge-Scope-Auflösung für feature-implizite Booleans
//! (explizite Zielkörper, "Merge with all", Warnungen).

use fielddraft_protocol::{BoolOp, FeatureId};

use super::{Context};
// ─── parameter helpers for the K1 primitives ─────────────────────

/// Resolve a "Merge scope" `targets` multi-query into explicit
/// `target_ids` (ticket 15f5): each `qCreatedBy(<feature_id>)` entry
/// names a seed feature whose body the boolean op folds into. An empty
/// list keeps the legacy AABB-overlap docking. Shared by every
/// boolean-capable feature (gyroid, helix, extrude, revolve).
pub(crate) fn target_ids_from_queries(
    targets: &::fielddraft_protocol::MultiQuery,
) -> Vec<FeatureId> {
    targets
        .queries()
        .iter()
        .filter_map(|q| {
            let inner = q.strip_prefix("qCreatedBy(")?.strip_suffix(')')?;
            inner.parse::<FeatureId>().ok()
        })
        .collect()
}

/// Resolve the effective merge-scope `target_ids` of a feature-implicit
/// boolean. `explicit` are the targets parsed from the "Merge scope" query
/// (may be empty); `merge_with_all` expands an empty scope into the seed id
/// of every body that exists *before* this feature in the spine. A resolved
/// scope routes the body through the explicit targeted-dock path in
/// [`crate::body::partition`] — touching solids never merge implicitly.
pub(crate) fn resolve_merge_scope(
    ctx: &Context<'_>,
    explicit: Vec<FeatureId>,
    merge_with_all: bool,
) -> Vec<FeatureId> {
    if !explicit.is_empty() {
        return explicit;
    }
    if merge_with_all {
        return ctx.current_body_seed_ids();
    }
    Vec::new()
}

/// Non-fatal warning for a feature-implicit boolean whose merge scope is
/// empty: `Remove`/`Intersect` need a target body (an explicit scope or
/// "Merge with all"), otherwise the operation has nothing to act on and the
/// body shows as a red translucent preview. `New` (a standalone body) is
/// always fine. A scope-less `Add` is fine **iff it is the first/standalone
/// solid** — like Onshape, the very first solid is implicitly "New"; only an
/// `Add` that *could* have merged into an existing body (but didn't pick one)
/// is flagged. See [`Context::merge_scope_warning`].
pub(crate) fn merge_scope_warning(op: BoolOp, resolved_targets: &[FeatureId], has_prior_bodies: bool) -> Option<String> {
    if op == BoolOp::New || !resolved_targets.is_empty() {
        return None;
    }
    // First/standalone Add: nothing to merge into yet → implicit New, no warning.
    if op == BoolOp::Add && !has_prior_bodies {
        return None;
    }
    Some(
        "Kein Merge-Scope gewählt — Add/Remove/Intersect braucht einen Zielkörper. \
         Aktiviere „Merge with all“ oder wähle einen oder mehrere Zielkörper, \
         sonst bleibt der Körper unverbunden."
            .to_string(),
    )
}
