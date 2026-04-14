//! Query minimum (K4.5.7).
//!
//! FieldDraft features reference geometry through **queries** rather
//! than raw entity ids — so a fillet parameter says "the edge between
//! feature `box_1` and feature `extrude_3`" rather than "edge with
//! mesh index 482". Queries survive regeneration: when upstream
//! features move or resize, the downstream fillet re-evaluates its
//! query against the new scene and still picks the geometrically
//! correct edge. This module is the first working slice of that
//! infrastructure.
//!
//! ## What K4.5.7 ships
//!
//! * The [`QueryExpr`] AST with the three query minimum forms the
//!   README commits to: `qCreatedBy(id)`, `qFaceOf(id)`, and
//!   `qEdgeBetween(id_a, id_b)`.
//! * [`parse_query`] — a tiny recursive-descent parser that accepts
//!   the canonical string form (`"qCreatedBy(box_1)"`) and returns
//!   a `QueryExpr` plus typed errors on malformed input.
//! * [`evaluate_query`] — a pure evaluator that walks a [`Document`]
//!   and returns a [`QueryResult`]. The evaluator operates at the
//!   **feature level**, not the mesh level: it checks feature
//!   existence and returns the contributor sets that satisfy the
//!   query, which is the semantic layer downstream features and
//!   the Outer API need. Full mesh-element resolution (clicking on
//!   a specific face / edge polyline) layers on top of this in a
//!   later slice once the query result type grows to carry proxy
//!   element handles.
//! * [`QueryCache`] — a version-keyed memoisation layer. When the
//!   context version bumps (typically because a regeneration step
//!   mutated the document), the cache invalidates every entry in
//!   one sweep; otherwise repeated queries hit the cache without
//!   re-walking the document.
//!
//! ## What K4.5.7 deliberately does *not* ship
//!
//! * Integration into [`Context`](crate::features::Context). The
//!   existing execute-path contract is intentionally unchanged in
//!   this slice — K4.5.5/4.5.6 already have their share of moving
//!   parts on the worker side, and threading a cache reference
//!   through every `FeatureKind::execute` call belongs in a
//!   dedicated follow-up. K4.5.7 stands the primitives up as a
//!   standalone module that the kernel and the Outer API can both
//!   reach for when they're ready.
//! * Mesh-level entity resolution. [`QueryResult`] carries the
//!   contributor `FeatureId` sets that match, not mesh vertex
//!   indices or proxy element handles. Downstream slices extend
//!   the result type when they need the geometric layer.
//! * Hierarchical queries (`qCreatedBy(extrude_3.op.mask_0)`).
//!   [`FeatureId`]'s hierarchical form is preserved through
//!   `FromStr`, so the parser already accepts the path syntax —
//!   the evaluator just treats every id as an opaque lookup key.

use std::collections::HashMap;
use std::fmt;

use fielddraft_protocol::{Document, FeatureId};

/// Parsed form of a `q…(…)` query string. Three variants match the
/// README's "query minimum" vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryExpr {
    /// `qCreatedBy(id)` — every entity whose provenance lists `id`
    /// as its sole contributor. Semantically: "the geometry this
    /// feature produced and nothing else touched".
    CreatedBy(FeatureId),
    /// `qFaceOf(id)` — the face(s) owned by a single feature. At
    /// the feature level this is the same set as `CreatedBy`, but
    /// the distinction is preserved so downstream slices can
    /// narrow to `Face`-classified proxy elements when the
    /// evaluator gains mesh-level resolution.
    FaceOf(FeatureId),
    /// `qEdgeBetween(a, b)` — edges that have both `a` and `b` (and
    /// only those two) in their contributor set. Corresponds to
    /// the emergent intersection curve between two primitives in
    /// the SDF proxy.
    EdgeBetween(FeatureId, FeatureId),
    /// K8.13: `qAllRegions(sketch_id)` — all closed regions of a sketch.
    AllRegions(FeatureId),
    /// K8.13: `qRegion(sketch_id, region_index)` — a single region.
    Region(FeatureId, u32),
    /// K8.13: `qRegions(sketch_id, [indices])` — a subset of regions.
    Regions(FeatureId, Vec<u32>),
}

impl fmt::Display for QueryExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryExpr::CreatedBy(id) => write!(f, "qCreatedBy({id})"),
            QueryExpr::FaceOf(id) => write!(f, "qFaceOf({id})"),
            QueryExpr::EdgeBetween(a, b) => write!(f, "qEdgeBetween({a}, {b})"),
            QueryExpr::AllRegions(id) => write!(f, "qAllRegions({id})"),
            QueryExpr::Region(id, idx) => write!(f, "qRegion({id}, {idx})"),
            QueryExpr::Regions(id, indices) => {
                let list = indices.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
                write!(f, "qRegions({id}, [{list}])")
            }
        }
    }
}

/// Errors the parser emits when a query string is malformed. Kept
/// as a small closed enum rather than a `String`-carrying variant
/// so callers can branch on the failure mode in tests and UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryParseError {
    /// The outer call name didn't match one of the three known
    /// query forms. Carries the offending name so error messages
    /// can be actionable.
    UnknownQuery(String),
    /// The parenthesised argument list was missing, unterminated,
    /// or had the wrong arity for the query name.
    MalformedArguments(&'static str),
    /// One of the `FeatureId` arguments failed to parse.
    InvalidFeatureId(String),
}

impl fmt::Display for QueryParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryParseError::UnknownQuery(name) => {
                write!(f, "unknown query `{name}`")
            }
            QueryParseError::MalformedArguments(msg) => {
                write!(f, "malformed query arguments: {msg}")
            }
            QueryParseError::InvalidFeatureId(raw) => {
                write!(f, "invalid feature id in query: `{raw}`")
            }
        }
    }
}

impl std::error::Error for QueryParseError {}

/// Parse a canonical query string into a [`QueryExpr`].
///
/// Accepts `qCreatedBy(<id>)`, `qFaceOf(<id>)`, and
/// `qEdgeBetween(<id_a>, <id_b>)`. Whitespace around arguments is
/// tolerated. Feature ids are parsed via [`FeatureId::from_str`],
/// so both flat-numeric (`1`, `42`) and hierarchical path forms
/// (`extrude_3.op.mask_0`) are accepted without the parser itself
/// knowing about the id shape.
pub fn parse_query(input: &str) -> Result<QueryExpr, QueryParseError> {
    let trimmed = input.trim();
    let (name, rest) = trimmed
        .split_once('(')
        .ok_or(QueryParseError::MalformedArguments(
            "expected '(' after query name",
        ))?;
    let args_raw = rest
        .strip_suffix(')')
        .ok_or(QueryParseError::MalformedArguments(
            "expected closing ')'",
        ))?;

    let args: Vec<&str> = if args_raw.trim().is_empty() {
        Vec::new()
    } else {
        args_raw.split(',').map(|s| s.trim()).collect()
    };

    match name.trim() {
        "qCreatedBy" => {
            let id = expect_one_id(&args, "qCreatedBy expects one id")?;
            Ok(QueryExpr::CreatedBy(id))
        }
        "qFaceOf" => {
            let id = expect_one_id(&args, "qFaceOf expects one id")?;
            Ok(QueryExpr::FaceOf(id))
        }
        "qEdgeBetween" => {
            let (a, b) = expect_two_ids(&args, "qEdgeBetween expects two ids")?;
            Ok(QueryExpr::EdgeBetween(a, b))
        }
        "qAllRegions" => {
            let id = expect_one_id(&args, "qAllRegions expects one sketch id")?;
            Ok(QueryExpr::AllRegions(id))
        }
        "qRegion" => {
            let (a, b) = expect_two_ids(&args, "qRegion expects sketch_id and region_index")?;
            let idx = b.as_u32_if_flat().ok_or(QueryParseError::InvalidFeatureId(
                "region index must be a plain number".to_string(),
            ))?;
            Ok(QueryExpr::Region(a, idx))
        }
        "qRegions" => {
            if args.len() < 2 {
                return Err(QueryParseError::MalformedArguments(
                    "qRegions expects sketch_id and [indices]",
                ));
            }
            let id = parse_feature_id(args[0])?;
            let mut indices = Vec::new();
            for arg in &args[1..] {
                let cleaned = arg.trim_matches(|c: char| c == '[' || c == ']' || c.is_whitespace());
                if cleaned.is_empty() { continue; }
                let n: u32 = cleaned.parse().map_err(|_| {
                    QueryParseError::InvalidFeatureId(format!("invalid region index: {cleaned}"))
                })?;
                indices.push(n);
            }
            Ok(QueryExpr::Regions(id, indices))
        }
        other => Err(QueryParseError::UnknownQuery(other.to_string())),
    }
}

fn expect_one_id(
    args: &[&str],
    message: &'static str,
) -> Result<FeatureId, QueryParseError> {
    if args.len() != 1 {
        return Err(QueryParseError::MalformedArguments(message));
    }
    parse_feature_id(args[0])
}

fn expect_two_ids(
    args: &[&str],
    message: &'static str,
) -> Result<(FeatureId, FeatureId), QueryParseError> {
    if args.len() != 2 {
        return Err(QueryParseError::MalformedArguments(message));
    }
    Ok((parse_feature_id(args[0])?, parse_feature_id(args[1])?))
}

fn parse_feature_id(raw: &str) -> Result<FeatureId, QueryParseError> {
    raw.parse::<FeatureId>()
        .map_err(|_| QueryParseError::InvalidFeatureId(raw.to_string()))
}

/// The high-level result of evaluating a query. Carries the
/// classification hint the query form implies plus the contributor
/// sets of every match. A match's contributor set is the list of
/// [`FeatureId`]s that jointly own the matched entity — for
/// `qCreatedBy` that's exactly one id, for `qEdgeBetween` it's two,
/// and downstream slices can grow the set as more query forms land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryResult {
    /// Which B-Rep proxy class the query is asking about. The
    /// feature-level evaluator in K4.5.7 doesn't actually consult
    /// the proxy yet, but carrying the hint lets downstream mesh-
    /// level resolution narrow the candidate set without a second
    /// query parse.
    pub kind: QueryElementKind,
    /// Each entry is the contributor set of one matching entity.
    /// Empty vec means "the query is well-formed but nothing in
    /// the document matches it right now".
    pub matches: Vec<Vec<FeatureId>>,
}

/// B-Rep proxy classification hint attached to a [`QueryResult`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryElementKind {
    /// `qCreatedBy` — everything the feature owns outright.
    Set,
    /// `qFaceOf` — face-classified proxy elements (single
    /// contributor).
    Face,
    /// `qEdgeBetween` — edge-classified proxy elements (two
    /// contributors exactly).
    Edge,
}

/// Evaluate a parsed query against a document.
///
/// At the feature level this is a three-line walk: check whether
/// each referenced [`FeatureId`] exists in
/// [`Document::features`]; if so, emit its contributor set as a
/// match. The result is deterministic for a given document state,
/// which is what the [`QueryCache`] relies on.
pub fn evaluate_query(document: &Document, expr: &QueryExpr) -> QueryResult {
    match expr {
        QueryExpr::CreatedBy(id) => QueryResult {
            kind: QueryElementKind::Set,
            matches: if feature_exists(document, id) {
                vec![vec![id.clone()]]
            } else {
                Vec::new()
            },
        },
        QueryExpr::FaceOf(id) => QueryResult {
            kind: QueryElementKind::Face,
            matches: if feature_exists(document, id) {
                vec![vec![id.clone()]]
            } else {
                Vec::new()
            },
        },
        QueryExpr::EdgeBetween(a, b) => QueryResult {
            kind: QueryElementKind::Edge,
            matches: if feature_exists(document, a) && feature_exists(document, b) {
                vec![vec![a.clone(), b.clone()]]
            } else {
                Vec::new()
            },
        },
        QueryExpr::AllRegions(id) | QueryExpr::Region(id, _) | QueryExpr::Regions(id, _) => {
            QueryResult {
                kind: QueryElementKind::Face,
                matches: if feature_exists(document, id) {
                    vec![vec![id.clone()]]
                } else {
                    Vec::new()
                },
            }
        }
    }
}

fn feature_exists(document: &Document, id: &FeatureId) -> bool {
    document.features.iter().any(|f| &f.id == id)
}

/// Version-keyed memoisation layer for query evaluation.
///
/// Callers pair the cache with a **context version** — typically
/// the mutation counter the kernel bumps on every feature spine
/// edit. When the version advances, every previously cached
/// result is stale and the cache wipes itself before the next
/// insert. Within a stable version, repeated lookups of the same
/// query string hit the memoised result without re-parsing or
/// re-walking the document.
///
/// The cache is intentionally global per kernel rather than per
/// regeneration pass — queries asked by the Outer API from the
/// main thread (e.g. to resolve a picker click) should see the
/// same memoised results as queries a feature body asks during
/// regeneration, as long as the mutation counter agrees.
#[derive(Debug, Clone, Default)]
pub struct QueryCache {
    version: u64,
    entries: HashMap<String, QueryResult>,
}

impl QueryCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The version the cache is currently keyed on. Starts at 0.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// How many entries are cached. Exposed for tests and the
    /// diagnostic surface; not load-bearing for correctness.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Advance the cache to a new context version. If the version
    /// actually changed, every entry is dropped — the caller is
    /// asserting that document mutations have invalidated prior
    /// results. No-op when the version matches the current one.
    pub fn set_version(&mut self, version: u64) {
        if version != self.version {
            self.entries.clear();
            self.version = version;
        }
    }

    /// Look up a query in the cache, evaluating and memoising on
    /// miss. Returns a reference to the cached [`QueryResult`].
    ///
    /// The `expr_str` is both the cache key and the parse input.
    /// A parse error propagates without polluting the cache, so a
    /// subsequent call with a corrected string is not blocked by
    /// the previous failure.
    pub fn get_or_evaluate(
        &mut self,
        document: &Document,
        expr_str: &str,
    ) -> Result<&QueryResult, QueryParseError> {
        if !self.entries.contains_key(expr_str) {
            let expr = parse_query(expr_str)?;
            let result = evaluate_query(document, &expr);
            self.entries.insert(expr_str.to_string(), result);
        }
        Ok(&self.entries[expr_str])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fielddraft_protocol::{BoolOp, Feature, FeatureId, Primitive, Transform};

    fn id(n: u32) -> FeatureId {
        FeatureId::from_u32(n)
    }

    fn box_feature(n: u32) -> Feature {
        Feature {
            id: id(n),
            name: format!("Box {n}"),
            op: BoolOp::Union,
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [1.0, 1.0, 1.0],
            },
            transform: Transform::IDENTITY,
            blend_radius: 0.0,
            chamfer_size: 0.0,
            twist_rate: 0.0,
            bend_rate: 0.0,
            displace_depth: 0.0,
            displace_freq: 0.0,
            shell_thickness: 0.0,
        }
    }

    fn doc_with_features(features: Vec<Feature>) -> Document {
        let max = features.iter().fold(0_u32, |acc, f| {
            f.id.as_u32_if_flat().map(|n| acc.max(n)).unwrap_or(acc)
        });
        Document {
            schema_version: fielddraft_protocol::SCHEMA_VERSION,
            name: "queries test".to_string(),
            features,
            feature_calls: Vec::new(),
            next_feature_id: max + 1,
            variables: std::collections::BTreeMap::new(),
        }
    }

    // ─── parse_query ──────────────────────────────────────────

    #[test]
    fn parses_created_by_with_flat_numeric_id() {
        let q = parse_query("qCreatedBy(1)").unwrap();
        assert_eq!(q, QueryExpr::CreatedBy(id(1)));
    }

    #[test]
    fn parses_face_of_with_flat_numeric_id() {
        let q = parse_query("qFaceOf(42)").unwrap();
        assert_eq!(q, QueryExpr::FaceOf(id(42)));
    }

    #[test]
    fn parses_edge_between_with_two_ids() {
        let q = parse_query("qEdgeBetween(1, 2)").unwrap();
        assert_eq!(q, QueryExpr::EdgeBetween(id(1), id(2)));
    }

    #[test]
    fn tolerates_whitespace_around_arguments() {
        let q = parse_query("  qEdgeBetween(  3 ,  4  )  ").unwrap();
        assert_eq!(q, QueryExpr::EdgeBetween(id(3), id(4)));
    }

    #[test]
    fn rejects_unknown_query_name() {
        let err = parse_query("qBogus(1)").unwrap_err();
        assert_eq!(err, QueryParseError::UnknownQuery("qBogus".to_string()));
    }

    #[test]
    fn rejects_missing_open_paren() {
        let err = parse_query("qCreatedBy 1").unwrap_err();
        assert!(matches!(err, QueryParseError::MalformedArguments(_)));
    }

    #[test]
    fn rejects_missing_close_paren() {
        let err = parse_query("qCreatedBy(1").unwrap_err();
        assert!(matches!(err, QueryParseError::MalformedArguments(_)));
    }

    #[test]
    fn rejects_wrong_arity_for_created_by() {
        let err = parse_query("qCreatedBy(1, 2)").unwrap_err();
        assert!(matches!(err, QueryParseError::MalformedArguments(_)));
    }

    #[test]
    fn rejects_wrong_arity_for_edge_between() {
        let err = parse_query("qEdgeBetween(1)").unwrap_err();
        assert!(matches!(err, QueryParseError::MalformedArguments(_)));
    }

    #[test]
    fn display_round_trips_every_variant() {
        for raw in [
            "qCreatedBy(1)",
            "qFaceOf(42)",
            "qEdgeBetween(1, 2)",
        ] {
            let parsed = parse_query(raw).unwrap();
            assert_eq!(parsed.to_string(), raw);
        }
    }

    // ─── evaluate_query ───────────────────────────────────────

    #[test]
    fn evaluate_created_by_finds_existing_feature() {
        let doc = doc_with_features(vec![box_feature(1), box_feature(2)]);
        let result = evaluate_query(&doc, &QueryExpr::CreatedBy(id(1)));
        assert_eq!(result.kind, QueryElementKind::Set);
        assert_eq!(result.matches, vec![vec![id(1)]]);
    }

    #[test]
    fn evaluate_created_by_returns_empty_for_missing_feature() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let result = evaluate_query(&doc, &QueryExpr::CreatedBy(id(999)));
        assert!(result.matches.is_empty());
        // Classification hint still travels through an empty
        // result — it's a property of the query, not the matches.
        assert_eq!(result.kind, QueryElementKind::Set);
    }

    #[test]
    fn evaluate_face_of_carries_face_kind_hint() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let result = evaluate_query(&doc, &QueryExpr::FaceOf(id(1)));
        assert_eq!(result.kind, QueryElementKind::Face);
        assert_eq!(result.matches, vec![vec![id(1)]]);
    }

    #[test]
    fn evaluate_edge_between_returns_both_ids_on_match() {
        let doc = doc_with_features(vec![box_feature(1), box_feature(2)]);
        let result =
            evaluate_query(&doc, &QueryExpr::EdgeBetween(id(1), id(2)));
        assert_eq!(result.kind, QueryElementKind::Edge);
        assert_eq!(result.matches, vec![vec![id(1), id(2)]]);
    }

    #[test]
    fn evaluate_edge_between_empty_when_one_feature_missing() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let result =
            evaluate_query(&doc, &QueryExpr::EdgeBetween(id(1), id(2)));
        assert!(result.matches.is_empty());
    }

    // ─── QueryCache ───────────────────────────────────────────

    #[test]
    fn cache_hits_return_identical_result_for_same_version() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let mut cache = QueryCache::new();
        let first = cache
            .get_or_evaluate(&doc, "qCreatedBy(1)")
            .unwrap()
            .clone();
        let second = cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        assert_eq!(&first, second);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_invalidates_on_version_bump() {
        let mut doc = doc_with_features(vec![box_feature(1)]);
        let mut cache = QueryCache::new();
        cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        assert_eq!(cache.len(), 1);

        // Simulate a mutation that removes feature 1.
        doc.features.clear();
        cache.set_version(1);
        assert_eq!(cache.len(), 0);

        // The next evaluation must reflect the new document state.
        let fresh = cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        assert!(fresh.matches.is_empty());
    }

    #[test]
    fn cache_set_version_is_noop_when_version_matches() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let mut cache = QueryCache::new();
        cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        cache.set_version(0);
        // Still cached — no invalidation happened.
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_parse_errors_do_not_pollute_the_table() {
        let doc = doc_with_features(vec![box_feature(1)]);
        let mut cache = QueryCache::new();
        let err = cache.get_or_evaluate(&doc, "qBogus(1)").unwrap_err();
        assert_eq!(err, QueryParseError::UnknownQuery("qBogus".to_string()));
        assert_eq!(cache.len(), 0);
        // A subsequent well-formed call still works.
        cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_tracks_multiple_entries_at_one_version() {
        let doc = doc_with_features(vec![box_feature(1), box_feature(2)]);
        let mut cache = QueryCache::new();
        cache.get_or_evaluate(&doc, "qCreatedBy(1)").unwrap();
        cache.get_or_evaluate(&doc, "qFaceOf(2)").unwrap();
        cache
            .get_or_evaluate(&doc, "qEdgeBetween(1, 2)")
            .unwrap();
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn cache_version_getter_reflects_set_version_calls() {
        let mut cache = QueryCache::new();
        assert_eq!(cache.version(), 0);
        cache.set_version(7);
        assert_eq!(cache.version(), 7);
        cache.set_version(7);
        assert_eq!(cache.version(), 7);
    }
}
