//! Merge-Scope-Auflösung für feature-implizite Booleans
//! (explizite Zielkörper, "Merge with all", Warnungen).

use fielddraft_protocol::{BoolOp, FeatureId};

use crate::csg::DecodedFeature;

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

/// Expand named body targets to the FULL set of member feature ids of every
/// owner body they name. A `qCreatedBy(f)` query points at *one* feature, but
/// a body assembled by Add-primitive docking (or a Boolean union) spreads
/// across several features, each contributing a sibling subtree under its OWN
/// feature id. A body-scoped post-process (offset, shell) must wrap the WHOLE
/// body tree, not just the named feature's branch — otherwise the docked
/// members are left at original size while only the named feature's subtree is
/// transformed (#399). Walk the current [`crate::body::partition`] and collect
/// every member's root id of each part that holds a named target.
///
/// `features` must be the decoded spine *before* the calling feature is pushed
/// (the part the target lives in must already exist). Returns the named ids
/// unchanged when no partition matches (a not-yet-present / standalone target).
pub(crate) fn expand_to_owner_body_members(
    features: &[DecodedFeature],
    named: &[FeatureId],
) -> Vec<FeatureId> {
    if named.is_empty() {
        return Vec::new();
    }
    let parts = crate::body::partition(features);
    let mut out: Vec<FeatureId> = Vec::new();
    for part in &parts {
        let holds_named = part.feature_indices.iter().any(|&fi| {
            let root = features[fi].id.root_ancestor();
            named.iter().any(|t| root == t)
        });
        if !holds_named {
            continue;
        }
        for &fi in &part.feature_indices {
            let root = features[fi].id.root_ancestor().clone();
            if !out.contains(&root) {
                out.push(root);
            }
        }
    }
    if out.is_empty() { named.to_vec() } else { out }
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
