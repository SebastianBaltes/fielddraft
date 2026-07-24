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
//! * [`resolve_query_topo`] / [`locate_element`] — the element-level
//!   resolver against the woven B-Reps (fingerprint healing included).
//!   A feature-level evaluator (`evaluate_query`) and a version-keyed
//!   `QueryCache` shipped with K4.5.7 but never gained a consumer and
//!   were removed (YAGNI, 2026-07-07) — the weave cache already keys
//!   the expensive work, and every real consumer needs geometry.
//!
//! ## Deliberately not shipped
//!
//! * Hierarchical queries (`qCreatedBy(extrude_3.op.mask_0)`).
//!   [`FeatureId`]'s hierarchical form is preserved through
//!   `FromStr`, so the parser already accepts the path syntax —
//!   the evaluator just treats every id as an opaque lookup key.

use std::fmt;

use fielddraft_protocol::FeatureId;


use crate::brep::naming::Fingerprint;

/// A persisted element reference: the stable `id_hash` plus the geometric
/// [`Fingerprint`] captured at pick time (plan `PERSISTENT-NAMING-PLAN.md`
/// N2, finally cashing in DL-35). The fingerprint is what lets
/// [`resolve_query_topo`] *heal* the reference after a topological edit —
/// without it, resolution is exact-hash-only (the legacy short form
/// `qFace(<hash>)`, still accepted).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElementRef {
    pub id_hash: u64,
    pub fingerprint: Option<Fingerprint>,
}

impl ElementRef {
    pub fn hash_only(id_hash: u64) -> Self {
        Self { id_hash, fingerprint: None }
    }
}

impl From<u64> for ElementRef {
    fn from(id_hash: u64) -> Self {
        Self::hash_only(id_hash)
    }
}

impl From<&crate::brep::naming::TopoRef> for ElementRef {
    fn from(r: &crate::brep::naming::TopoRef) -> Self {
        Self { id_hash: r.id_hash, fingerprint: Some(r.fingerprint) }
    }
}

/// The role a B-Rep face plays within its owning feature — a pure
/// formalisation of the sub-id bit layout that has always been there
/// (`BranchId::EXTRUDE_CAP_TOP_SUB` / `…_BOTTOM_SUB` /
/// `INNER_WALL_FLAG`; blend carriers are recognised by their
/// [`HalfSpace::Blend`](fielddraft_protocol::HalfSpace) surface, since a
/// fillet mints its faces at sub 0). Nothing about the layout moves —
/// existing id hashes are untouched. The role vocabulary is what the
/// Onshape import needs to translate `CAP_FACE` / `SWEPT_FACE` /
/// `BLEND_FACE` creation queries onto FieldDraft faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceRole {
    /// A lateral sweep wall (sketch boundary group) or a primitive's
    /// ordinary sub-face — everything the reserved sub-ids don't claim.
    Wall,
    /// The extrude/helix end cap on the +axis side (`EXTRUDE_CAP_TOP_SUB`).
    CapTop,
    /// The end cap on the −axis side (`EXTRUDE_CAP_BOTTOM_SUB`).
    CapBottom,
    /// A shell inner-wall face — the inward offset of a lateral wall
    /// (`INNER_WALL_FLAG`, DL-75).
    InnerWall,
    /// The inward offset of the +axis cap — a shelled box's inner ceiling.
    /// Distinct from [`InnerWall`](Self::InnerWall): the edge where a
    /// hollow's floor meets its side is `INNER_CAP_* ∩ INNER_WALL`, so
    /// lumping the two would make that edge unaddressable.
    InnerCapTop,
    /// The inward offset of the −axis cap — a shelled box's inner floor.
    InnerCapBottom,
    /// A fillet/chamfer blend carrier face (`HalfSpace::Blend` surface).
    Blend,
}

impl FaceRole {
    /// Canonical token used in the query string form (`qCreatedBy(3, CAP_TOP)`).
    pub fn token(self) -> &'static str {
        match self {
            FaceRole::Wall => "WALL",
            FaceRole::CapTop => "CAP_TOP",
            FaceRole::CapBottom => "CAP_BOTTOM",
            FaceRole::InnerWall => "INNER_WALL",
            FaceRole::InnerCapTop => "INNER_CAP_TOP",
            FaceRole::InnerCapBottom => "INNER_CAP_BOTTOM",
            FaceRole::Blend => "BLEND",
        }
    }

    /// Parse the canonical token (case-sensitive — the string form is a
    /// wire format, not user input).
    pub fn from_token(token: &str) -> Option<Self> {
        Some(match token {
            "WALL" => FaceRole::Wall,
            "CAP_TOP" => FaceRole::CapTop,
            "CAP_BOTTOM" => FaceRole::CapBottom,
            "INNER_WALL" => FaceRole::InnerWall,
            "INNER_CAP_TOP" => FaceRole::InnerCapTop,
            "INNER_CAP_BOTTOM" => FaceRole::InnerCapBottom,
            "BLEND" => FaceRole::Blend,
            _ => return None,
        })
    }
}

impl fmt::Display for FaceRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.token())
    }
}

/// Classify the role a face plays within its owning feature — the single
/// authority mapping the existing bit layout (and the blend surface
/// class) onto [`FaceRole`]. Blend wins first: a fillet's carrier face is
/// minted at sub 0, which would otherwise read as `Wall`.
pub fn face_role(face: &crate::brep::Face) -> FaceRole {
    use crate::brep::SurfaceGeometry;
    use crate::csg::BranchId;
    use fielddraft_protocol::HalfSpace;
    if let SurfaceGeometry::Analytic { surface: HalfSpace::Blend { .. }, .. } = &face.surface {
        return FaceRole::Blend;
    }
    let branch = face.branch_id;
    // The inner-wall flag rides ON TOP of the outer sub-id, so an inner CAP
    // still carries the reserved cap sub — strip the flag before classifying.
    let sub = branch.sub() & !BranchId::INNER_WALL_FLAG;
    let cap = match sub {
        BranchId::EXTRUDE_CAP_TOP_SUB => Some(true),
        BranchId::EXTRUDE_CAP_BOTTOM_SUB => Some(false),
        _ => None,
    };
    match (branch.is_inner_wall(), cap) {
        (true, Some(true)) => FaceRole::InnerCapTop,
        (true, Some(false)) => FaceRole::InnerCapBottom,
        (true, None) => FaceRole::InnerWall,
        (false, Some(true)) => FaceRole::CapTop,
        (false, Some(false)) => FaceRole::CapBottom,
        (false, None) => FaceRole::Wall,
    }
}

/// Parsed form of a `q…(…)` query string. Three variants match the
/// README's "query minimum" vocabulary.
///
/// No `Eq`: element references carry `f64` fingerprint data.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryExpr {
    /// `qCreatedBy(id)` — every entity whose provenance lists `id`
    /// as its sole contributor. Semantically: "the geometry this
    /// feature produced and nothing else touched".
    CreatedBy(FeatureId),
    /// `qCreatedBy(id, ROLE)` — the faces feature `id` produced,
    /// narrowed to one [`FaceRole`] (`qCreatedBy(3, CAP_TOP)`). The
    /// role-filtered sibling of [`CreatedBy`](Self::CreatedBy); the
    /// Onshape import's translation target for `CAP_FACE` /
    /// `SWEPT_FACE` / `BLEND_FACE` creation queries.
    CreatedByRole(FeatureId, FaceRole),
    /// `qWall(op, entity[, INNER])` — the lateral wall face of sweep feature
    /// `op` swept from the sketch entity with stable ordinal `entity` (the
    /// branch sub-id minted from the sweep's `wall_subs` table — naming
    /// schema v2). The translation target for Onshape's `SWEPT_FACE`
    /// queries, whose origin is a sketch entity.
    ///
    /// The `INNER` modifier selects that wall's SHELL COUNTERPART — the
    /// inward-offset face of the hollow, which is a different face of the
    /// same sweep. Onshape names it `OFFSET_FACE`, and an extrude that runs
    /// "up to the inner wall" needs exactly this.
    Wall(FeatureId, u32, bool),
    /// `qIntersection(a, b)` — the elements matched by BOTH sub-queries
    /// (set intersection by identity). Onshape disambiguation queries
    /// compose provenance with topology filters; this is the composition
    /// node. Nests arbitrarily.
    Intersection(Box<QueryExpr>, Box<QueryExpr>),
    /// `qCommonEdge(a, b)` — the B-Rep edges shared by a face matched
    /// by `a` and a face matched by `b`. The import translation for
    /// Onshape's INTERSECT edge queries (a fillet edge named as "the
    /// edge where the cap meets this swept wall").
    CommonEdge(Box<QueryExpr>, Box<QueryExpr>),
    /// `qAdjacentTo(a, b)` — the faces matched by `a` that share at
    /// least one B-Rep edge with a face matched by `b`. The FieldDraft
    /// analogue of Onshape's TOPOLOGY-neighbourhood disambiguation
    /// (`SWEPT_EDGE` → `TRUE_DEPENDENCY` → `BLEND_FACE`, boolean
    /// `SPLIT` fragments): after a boolean splits a swept wall into
    /// several faces of the same branch, adjacency to a named
    /// neighbour picks the right fragment.
    AdjacentTo(Box<QueryExpr>, Box<QueryExpr>),
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
    /// `qFace(id_hash [@ fp])` — the single B-Rep face whose stable
    /// [`TopoRef::id_hash`](crate::brep::naming::TopoRef) equals the
    /// given value. This is the id_hash-addressed form the UI emits
    /// when a user clicks exactly ONE face, and that a feature stores
    /// to reference it across regenerations. The optional `@`-suffixed
    /// fingerprint block (`cx cy cz size`; edges append `tx ty tz`)
    /// enables healing when the hash no longer resolves exactly.
    Face(ElementRef),
    /// `qEdge(id_hash [@ fp])` — the single B-Rep edge addressed by its
    /// stable id_hash, optionally fingerprint-carrying.
    Edge(ElementRef),
    /// `qVertex(id_hash [@ fp])` — the single B-Rep vertex addressed by
    /// its stable id_hash, optionally fingerprint-carrying.
    Vertex(ElementRef),
    /// `qSketchLine(sketch_id, line_id)` — one straight `sketch_line`
    /// sub-call of a committed sketch, addressed by the sketch feature
    /// id and the line's stable entity id. Unlike `qEdge`, this targets
    /// raw sketch geometry (including construction lines, which never
    /// reach the B-Rep) — the form the Revolve tool stores when the user
    /// picks a sketch line as the revolve axis.
    SketchLine(FeatureId, u32),
    /// `qSketchPoint(sketch_id, entity_id)` — one point entity of a committed
    /// sketch, addressed by the sketch feature id and the point's stable solver
    /// [`EntityId`](fielddraft_solver::EntityId). Unlike `qVertex`, this targets
    /// raw sketch geometry: a free sketch point never enters the woven B-Rep, so
    /// it has no vertex to address. The entity id is the shared Kernel↔Web
    /// sketch entity-id space (the kernel's `SketchState::with_origin`
    /// allocation order matches the web's `entityIdForGrip`), so the reference
    /// survives regeneration and document reordering. The form the Hole tool
    /// stores when the user picks a sketch point as a bore location.
    SketchPoint(FeatureId, u32),
}

impl fmt::Display for QueryExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryExpr::CreatedBy(id) => write!(f, "qCreatedBy({id})"),
            QueryExpr::CreatedByRole(id, role) => write!(f, "qCreatedBy({id}, {role})"),
            QueryExpr::Wall(id, entity, inner) => {
                if *inner {
                    write!(f, "qWall({id}, {entity}, INNER)")
                } else {
                    write!(f, "qWall({id}, {entity})")
                }
            }
            QueryExpr::Intersection(a, b) => write!(f, "qIntersection({a}, {b})"),
            QueryExpr::AdjacentTo(a, b) => write!(f, "qAdjacentTo({a}, {b})"),
            QueryExpr::CommonEdge(a, b) => write!(f, "qCommonEdge({a}, {b})"),
            QueryExpr::FaceOf(id) => write!(f, "qFaceOf({id})"),
            QueryExpr::EdgeBetween(a, b) => write!(f, "qEdgeBetween({a}, {b})"),
            QueryExpr::AllRegions(id) => write!(f, "qAllRegions({id})"),
            QueryExpr::Region(id, idx) => write!(f, "qRegion({id}, {idx})"),
            QueryExpr::Regions(id, indices) => {
                let list = indices.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
                write!(f, "qRegions({id}, [{list}])")
            }
            QueryExpr::Face(r) => write_element_ref(f, "qFace", r),
            QueryExpr::Edge(r) => write_element_ref(f, "qEdge", r),
            QueryExpr::Vertex(r) => write_element_ref(f, "qVertex", r),
            QueryExpr::SketchLine(sketch, line) => write!(f, "qSketchLine({sketch}, {line})"),
            QueryExpr::SketchPoint(sketch, point) => write!(f, "qSketchPoint({sketch}, {point})"),
        }
    }
}

/// Canonical string form of an element reference: `qFace(<hash>)` without a
/// fingerprint, `qFace(<hash> @ cx cy cz size [tx ty tz])` with one. Rust's
/// `f64` Display is shortest-round-trip, so the floats survive the string
/// round trip bit-exactly. The tangent triple is only emitted when non-zero
/// (edges); faces/vertices stay at four numbers.
fn write_element_ref(f: &mut fmt::Formatter<'_>, name: &str, r: &ElementRef) -> fmt::Result {
    match &r.fingerprint {
        None => write!(f, "{name}({})", r.id_hash),
        Some(fp) => {
            write!(
                f,
                "{name}({} @ {} {} {} {}",
                r.id_hash, fp.centroid[0], fp.centroid[1], fp.centroid[2], fp.size
            )?;
            if fp.tangent.iter().any(|&t| t != 0.0) {
                write!(f, " {} {} {}", fp.tangent[0], fp.tangent[1], fp.tangent[2])?;
            }
            write!(f, ")")
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
    /// An id_hash argument (`qFace`/`qEdge`/`qVertex`) was not a valid
    /// `u64` decimal. Carries the offending token.
    InvalidIdHash(String),
    /// The `@`-suffixed fingerprint block of an element query was not a
    /// list of 4 (face/vertex) or 7 (edge) floats. Carries the block.
    InvalidFingerprint(String),
    /// The role argument of `qCreatedBy(id, ROLE)` was not a known
    /// [`FaceRole`] token. Carries the offending token.
    InvalidRole(String),
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
            QueryParseError::InvalidIdHash(raw) => {
                write!(f, "invalid id_hash in query: `{raw}`")
            }
            QueryParseError::InvalidFingerprint(raw) => {
                write!(f, "invalid fingerprint block in query: `{raw}`")
            }
            QueryParseError::InvalidRole(raw) => {
                write!(
                    f,
                    "invalid face role `{raw}` (expected WALL, CAP_TOP, CAP_BOTTOM, INNER_WALL, INNER_CAP_TOP, INNER_CAP_BOTTOM or BLEND)"
                )
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

    // Paren-aware top-level split: composed queries (`qIntersection`,
    // `qAdjacentTo`) carry whole sub-queries as arguments, whose own
    // commas must not split the outer argument list.
    let args: Vec<&str> = if args_raw.trim().is_empty() {
        Vec::new()
    } else {
        split_top_level(args_raw)
    };

    match name.trim() {
        "qCreatedBy" => match args.len() {
            1 => Ok(QueryExpr::CreatedBy(parse_feature_id(args[0])?)),
            2 => {
                let id = parse_feature_id(args[0])?;
                let role = FaceRole::from_token(args[1])
                    .ok_or_else(|| QueryParseError::InvalidRole(args[1].to_string()))?;
                Ok(QueryExpr::CreatedByRole(id, role))
            }
            _ => Err(QueryParseError::MalformedArguments(
                "qCreatedBy expects one id, optionally followed by a face role",
            )),
        },
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
        "qFace" => Ok(QueryExpr::Face(expect_one_element_ref(
            &args,
            "qFace expects one id_hash",
        )?)),
        "qEdge" => Ok(QueryExpr::Edge(expect_one_element_ref(
            &args,
            "qEdge expects one id_hash",
        )?)),
        "qVertex" => Ok(QueryExpr::Vertex(expect_one_element_ref(
            &args,
            "qVertex expects one id_hash",
        )?)),
        "qWall" => {
            if args.len() != 2 && args.len() != 3 {
                return Err(QueryParseError::MalformedArguments(
                    "qWall expects op_id, entity ordinal and an optional INNER",
                ));
            }
            let a = parse_feature_id(args[0])?;
            let entity = parse_feature_id(args[1])?
                .as_u32_if_flat()
                .ok_or(QueryParseError::InvalidFeatureId(
                    "wall entity ordinal must be a plain number".to_string(),
                ))?;
            let inner = match args.get(2) {
                None => false,
                Some(&"INNER") => true,
                Some(other) => return Err(QueryParseError::InvalidRole((*other).to_string())),
            };
            Ok(QueryExpr::Wall(a, entity, inner))
        }
        "qIntersection" => {
            let (a, b) = expect_two_subqueries(&args, "qIntersection expects two queries")?;
            Ok(QueryExpr::Intersection(Box::new(a), Box::new(b)))
        }
        "qAdjacentTo" => {
            let (a, b) = expect_two_subqueries(&args, "qAdjacentTo expects two queries")?;
            Ok(QueryExpr::AdjacentTo(Box::new(a), Box::new(b)))
        }
        "qCommonEdge" => {
            let (a, b) = expect_two_subqueries(&args, "qCommonEdge expects two queries")?;
            Ok(QueryExpr::CommonEdge(Box::new(a), Box::new(b)))
        }
        "qSketchLine" => {
            let (a, b) = expect_two_ids(&args, "qSketchLine expects sketch_id and line_id")?;
            let line = b.as_u32_if_flat().ok_or(QueryParseError::InvalidFeatureId(
                "line id must be a plain number".to_string(),
            ))?;
            Ok(QueryExpr::SketchLine(a, line))
        }
        "qSketchPoint" => {
            let (a, b) = expect_two_ids(&args, "qSketchPoint expects sketch_id and entity_id")?;
            let point = b.as_u32_if_flat().ok_or(QueryParseError::InvalidFeatureId(
                "entity id must be a plain number".to_string(),
            ))?;
            Ok(QueryExpr::SketchPoint(a, point))
        }
        other => Err(QueryParseError::UnknownQuery(other.to_string())),
    }
}

/// Split an argument list on TOP-LEVEL commas only — commas inside
/// nested `q…(…)` sub-queries stay with their argument.
fn split_top_level(args_raw: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, ch) in args_raw.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(args_raw[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(args_raw[start..].trim());
    out
}

fn expect_two_subqueries(
    args: &[&str],
    message: &'static str,
) -> Result<(QueryExpr, QueryExpr), QueryParseError> {
    if args.len() != 2 {
        return Err(QueryParseError::MalformedArguments(message));
    }
    Ok((parse_query(args[0])?, parse_query(args[1])?))
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

/// Parse the single argument of a `qFace`/`qEdge`/`qVertex` query as an
/// [`ElementRef`]: a `u64` id_hash, optionally followed by an `@`-separated
/// fingerprint block of 4 (face/vertex) or 7 (edge) whitespace-separated
/// floats. The hash is parsed as `u64` (not `i64`) because an id_hash is a
/// full 64-bit FNV-1a value that routinely exceeds `i64::MAX`.
fn expect_one_element_ref(
    args: &[&str],
    message: &'static str,
) -> Result<ElementRef, QueryParseError> {
    if args.len() != 1 {
        return Err(QueryParseError::MalformedArguments(message));
    }
    let (hash_raw, fp_raw) = match args[0].split_once('@') {
        Some((h, fp)) => (h.trim(), Some(fp.trim())),
        None => (args[0], None),
    };
    let id_hash = hash_raw
        .parse::<u64>()
        .map_err(|_| QueryParseError::InvalidIdHash(hash_raw.to_string()))?;
    let fingerprint = match fp_raw {
        None => None,
        Some(block) => {
            let nums: Result<Vec<f64>, _> =
                block.split_whitespace().map(|t| t.parse::<f64>()).collect();
            let nums =
                nums.map_err(|_| QueryParseError::InvalidFingerprint(block.to_string()))?;
            let tangent = match nums.len() {
                4 => [0.0; 3],
                7 => [nums[4], nums[5], nums[6]],
                _ => return Err(QueryParseError::InvalidFingerprint(block.to_string())),
            };
            Some(Fingerprint {
                centroid: [nums[0], nums[1], nums[2]],
                size: nums[3],
                tangent,
            })
        }
    };
    Ok(ElementRef { id_hash, fingerprint })
}

/// The element-level query resolver. Where the (removed, YAGNI) feature-
/// level evaluator only echoed contributor ids, this
/// answers "does feature `f` exist and what owns the match", this drops
/// down onto the live solid and returns the actual faces/edges the
/// feature produced, as [`TopoRef`](crate::brep::naming::TopoRef)
/// handles whose `id_hash` survives a re-weave (the persistent-naming
/// guarantee from `brep::naming`).
///
/// Semantics — a [`BranchId`](crate::csg::BranchId) is mapped back to
/// its flat feature id via [`BranchId::feature_id`], compared against
/// the query's [`FeatureId::as_u32_or_root`]:
/// - `CreatedBy(f)` / `FaceOf(f)`: every [`Face`](crate::brep::Face)
///   whose `branch_id.feature_id() == f` → [`TopoRef::from_face`]. The
///   two are kept as distinct records of intent even though they
///   resolve identically today (CreatedBy may later widen to the
///   edges/vertices touching `f`).
/// - `EdgeBetween(a, b)`: every [`Edge`](crate::brep::Edge) whose two
///   `signature` branches map to exactly the unordered pair `{a, b}` →
///   [`TopoRef::from_edge`].
/// - `AllRegions/Region/Regions`: sketch-region queries (K8.13). These
///   resolve against **sketch** data, not the woven solid, so there is
///   nothing to find in a `BRep` — they return `Vec::new()`.
///
/// Spanning multiple B-Reps (one per body partition) is supported: the
/// arenas are walked per-brep and the union returned. Output is sorted
/// by `id_hash` (then by kind) so the result is deterministic across
/// calls and across body iteration order.
///
/// A query feature id that is not flat-or-root (`as_u32_or_root()` is
/// `None`) matches nothing — hierarchical clone ids aren't yet mapped
/// to branch feature ids here (follow-up alongside hierarchical query
/// support).
pub fn resolve_query_topo(
    breps: &[crate::brep::BRep],
    expr: &QueryExpr,
) -> Vec<crate::brep::naming::TopoRef> {
    use crate::brep::naming::TopoRef;
    use crate::brep::{EdgeId, FaceId};

    let mut out: Vec<TopoRef> = Vec::new();

    match expr {
        QueryExpr::CreatedBy(f) | QueryExpr::FaceOf(f) => {
            let Some(target) = f.as_u32_or_root() else {
                return Vec::new();
            };
            for brep in breps {
                for i in 0..brep.faces.len() {
                    if brep.faces[i].branch_id.feature_id() == target {
                        out.push(TopoRef::from_face(brep, FaceId(i as u32)));
                    }
                }
            }
        }
        QueryExpr::CreatedByRole(f, role) => {
            let Some(target) = f.as_u32_or_root() else {
                return Vec::new();
            };
            for brep in breps {
                for i in 0..brep.faces.len() {
                    let face = &brep.faces[i];
                    if face.branch_id.feature_id() == target && face_role(face) == *role {
                        out.push(TopoRef::from_face(brep, FaceId(i as u32)));
                    }
                }
            }
        }
        QueryExpr::Wall(f, entity, inner) => {
            let Some(target) = f.as_u32_or_root() else {
                return Vec::new();
            };
            let want = if *inner { FaceRole::InnerWall } else { FaceRole::Wall };
            for brep in breps {
                for i in 0..brep.faces.len() {
                    let face = &brep.faces[i];
                    let b = face.branch_id;
                    // An inner wall carries the SAME entity ordinal with the
                    // inner-wall flag set — mask it off before comparing.
                    let sub = b.sub() & !crate::csg::BranchId::INNER_WALL_FLAG;
                    if b.feature_id() == target && sub == *entity && face_role(face) == want {
                        out.push(TopoRef::from_face(brep, FaceId(i as u32)));
                    }
                }
            }
        }
        QueryExpr::Intersection(a, b) => {
            let b_ids: std::collections::HashSet<u64> = resolve_query_topo(breps, b)
                .into_iter()
                .map(|r| r.id_hash)
                .collect();
            out = resolve_query_topo(breps, a)
                .into_iter()
                .filter(|r| b_ids.contains(&r.id_hash))
                .collect();
        }
        QueryExpr::CommonEdge(a, b) => {
            use crate::brep::naming::EntityKind;
            use std::collections::HashSet;
            let a_ids: HashSet<u64> = resolve_query_topo(breps, a)
                .into_iter()
                .filter(|r| r.kind == EntityKind::Face)
                .map(|r| r.id_hash)
                .collect();
            let b_ids: HashSet<u64> = resolve_query_topo(breps, b)
                .into_iter()
                .filter(|r| r.kind == EntityKind::Face)
                .map(|r| r.id_hash)
                .collect();
            for brep in breps {
                let refs: Vec<TopoRef> = (0..brep.faces.len())
                    .map(|i| TopoRef::from_face(brep, FaceId(i as u32)))
                    .collect();
                let edges_of = |i: usize| -> std::collections::HashSet<u32> {
                    let f = &brep.faces[i];
                    f.outer
                        .edges
                        .iter()
                        .chain(f.inner.iter().flat_map(|l| l.edges.iter()))
                        .map(|(e, _)| e.0)
                        .collect()
                };
                let mut shared: Vec<u32> = Vec::new();
                for i in 0..brep.faces.len() {
                    if !a_ids.contains(&refs[i].id_hash) {
                        continue;
                    }
                    let ei = edges_of(i);
                    for j in 0..brep.faces.len() {
                        if j == i || !b_ids.contains(&refs[j].id_hash) {
                            continue;
                        }
                        for e in ei.intersection(&edges_of(j)) {
                            shared.push(*e);
                        }
                    }
                }
                shared.sort_unstable();
                shared.dedup();
                for e in shared {
                    out.push(TopoRef::from_edge(brep, EdgeId(e)));
                }
            }
        }
        QueryExpr::AdjacentTo(a, b) => {
            use crate::brep::naming::EntityKind;
            use std::collections::HashSet;
            let a_ids: HashSet<u64> = resolve_query_topo(breps, a)
                .into_iter()
                .filter(|r| r.kind == EntityKind::Face)
                .map(|r| r.id_hash)
                .collect();
            let b_ids: HashSet<u64> = resolve_query_topo(breps, b)
                .into_iter()
                .filter(|r| r.kind == EntityKind::Face)
                .map(|r| r.id_hash)
                .collect();
            for brep in breps {
                // Face → boundary-edge sets, once per body.
                let face_edges: Vec<HashSet<u32>> = brep
                    .faces
                    .iter()
                    .map(|f| {
                        f.outer
                            .edges
                            .iter()
                            .chain(f.inner.iter().flat_map(|l| l.edges.iter()))
                            .map(|(e, _)| e.0)
                            .collect()
                    })
                    .collect();
                let refs: Vec<TopoRef> = (0..brep.faces.len())
                    .map(|i| TopoRef::from_face(brep, FaceId(i as u32)))
                    .collect();
                for i in 0..brep.faces.len() {
                    if !a_ids.contains(&refs[i].id_hash) {
                        continue;
                    }
                    let touches_b = (0..brep.faces.len()).any(|j| {
                        j != i
                            && b_ids.contains(&refs[j].id_hash)
                            && !face_edges[i].is_disjoint(&face_edges[j])
                    });
                    if touches_b {
                        out.push(refs[i]);
                    }
                }
            }
        }
        QueryExpr::EdgeBetween(a, b) => {
            let (Some(fa), Some(fb)) = (a.as_u32_or_root(), b.as_u32_or_root()) else {
                return Vec::new();
            };
            for brep in breps {
                for i in 0..brep.edges.len() {
                    let sig = brep.edges[i].signature;
                    let s0 = sig.0.feature_id();
                    let s1 = sig.1.feature_id();
                    // Unordered pair match: {s0, s1} == {fa, fb}.
                    if (s0 == fa && s1 == fb) || (s0 == fb && s1 == fa) {
                        out.push(TopoRef::from_edge(brep, EdgeId(i as u32)));
                    }
                }
            }
        }
        // id_hash-addressed single-element queries. Two regimes
        // (plan PERSISTENT-NAMING-PLAN.md N3):
        //
        // - **Fingerprint-carrying reference** (the canonical long form):
        //   run the full `naming::resolve` state machine per body and bind
        //   to the best `Resolved`/`Healed` outcome. This both survives
        //   topological edits (healing) and refuses the swapped-hash-carrier
        //   misbinding (the resolver's geometric override). `Ambiguous` and
        //   `Missing` yield nothing — the caller reports the reference as
        //   unresolved instead of acting on an arbitrary pick.
        // - **Hash-only reference** (legacy short form, or programmatic
        //   `fd.q.face(hash)`): exact hash scan, as before — there is no
        //   stored geometry to heal against.
        QueryExpr::Face(eref) => {
            resolve_element(breps, crate::brep::naming::EntityKind::Face, eref, &mut out)
        }
        QueryExpr::Edge(eref) => {
            resolve_element(breps, crate::brep::naming::EntityKind::Edge, eref, &mut out)
        }
        QueryExpr::Vertex(eref) => {
            resolve_element(breps, crate::brep::naming::EntityKind::Vertex, eref, &mut out)
        }
        // Sketch-region and sketch-line queries resolve against sketch
        // data, not the woven solid; there is no B-Rep element to hand back.
        QueryExpr::AllRegions(_)
        | QueryExpr::Region(_, _)
        | QueryExpr::Regions(_, _)
        | QueryExpr::SketchLine(_, _)
        | QueryExpr::SketchPoint(_, _) => {}
    }

    // Deterministic order: by id_hash, then kind as a stable tie-break.
    out.sort_by(|x, y| {
        x.id_hash
            .cmp(&y.id_hash)
            .then((x.kind as u8).cmp(&(y.kind as u8)))
    });
    out
}

/// Resolve one id_hash-addressed element reference against all bodies —
/// the two-regime logic described at the `Face`/`Edge`/`Vertex` arms of
/// [`resolve_query_topo`].
fn resolve_element(
    breps: &[crate::brep::BRep],
    kind: crate::brep::naming::EntityKind,
    eref: &ElementRef,
    out: &mut Vec<crate::brep::naming::TopoRef>,
) {
    use crate::brep::naming::{EntityHandle, TopoRef};
    if let Some((bi, handle)) = locate_element(breps.iter(), kind, eref) {
        let brep = &breps[bi];
        out.push(match handle {
            EntityHandle::Face(id) => TopoRef::from_face(brep, id),
            EntityHandle::Edge(id) => TopoRef::from_edge(brep, id),
            EntityHandle::Vertex(id) => TopoRef::from_vertex(brep, id),
        });
    }
}

/// The canonical fingerprint-carrying element query for a [`TopoRef`] —
/// what a viewport pick should persist so the reference can heal later.
/// The single authority for the string form is [`QueryExpr`]'s `Display`.
pub fn element_query(r: &crate::brep::naming::TopoRef) -> QueryExpr {
    use crate::brep::naming::EntityKind;
    let eref = ElementRef::from(r);
    match r.kind {
        EntityKind::Face => QueryExpr::Face(eref),
        EntityKind::Edge => QueryExpr::Edge(eref),
        EntityKind::Vertex => QueryExpr::Vertex(eref),
    }
}

/// Locate the single element an [`ElementRef`] names across `breps`,
/// returning `(body index, arena handle)`. The shared primitive behind
/// [`resolve_query_topo`] and every `Context::resolve_*` face/edge/vertex
/// path — one implementation of the fingerprint-healing regime instead of
/// a hash-scan copy per feature.
///
/// - With a fingerprint: run [`naming::resolve`](resolve) per body and take
///   the best `Resolved`/`Healed` outcome (`Ambiguous`/`Missing` → `None`).
/// - Hash-only: first exact `id_hash` match wins, as before.
pub fn locate_element<'a, I>(
    breps: I,
    kind: crate::brep::naming::EntityKind,
    eref: &ElementRef,
) -> Option<(usize, crate::brep::naming::EntityHandle)>
where
    I: IntoIterator<Item = &'a crate::brep::BRep>,
{
    use crate::brep::naming::{resolve, EntityHandle, EntityKind, ResolutionState, TopoRef};
    use crate::brep::{EdgeId, FaceId, VertexId};

    match eref.fingerprint {
        Some(fingerprint) => {
            let stored = TopoRef { kind, id_hash: eref.id_hash, fingerprint };
            // Identity is provenance-first ACROSS bodies too (the same rule
            // `naming::resolve` applies within one body): when some body still
            // carries the exact `id_hash`, resolution is confined to those
            // bodies. Without this, a fingerprint-identical twin in another
            // body — two equal boxes placed apart share every body-LOCAL
            // fingerprint — scores a perfect geometric match (1.0) in the
            // no-hash branch, ties the hash carrier's score, and the strict
            // `>` keeps whichever body enumerates first: both of a loft's
            // profile queries then land on the SAME face ("two loft profiles
            // lie on the same plane", #385 P4). Cross-body healing still runs
            // when the hash survives nowhere.
            let all: Vec<&crate::brep::BRep> = breps.into_iter().collect();
            let contains_hash = |brep: &crate::brep::BRep| -> bool {
                let n = match kind {
                    EntityKind::Face => brep.faces.len(),
                    EntityKind::Edge => brep.edges.len(),
                    EntityKind::Vertex => brep.vertices.len(),
                };
                (0..n).any(|i| {
                    let r = match kind {
                        EntityKind::Face => TopoRef::from_face(brep, FaceId(i as u32)),
                        EntityKind::Edge => TopoRef::from_edge(brep, EdgeId(i as u32)),
                        EntityKind::Vertex => TopoRef::from_vertex(brep, VertexId(i as u32)),
                    };
                    r.id_hash == eref.id_hash
                })
            };
            let has_hash: Vec<bool> = all.iter().map(|b| contains_hash(b)).collect();
            let any_hash = has_hash.iter().any(|&h| h);
            let mut best: Option<(f64, usize, EntityHandle)> = None;
            for (bi, brep) in all.iter().enumerate() {
                if any_hash && !has_hash[bi] {
                    continue;
                }
                let res = resolve(&stored, brep);
                if !matches!(res.state, ResolutionState::Resolved | ResolutionState::Healed) {
                    continue;
                }
                let Some(handle) = res.handle else { continue };
                if best.as_ref().is_none_or(|(s, _, _)| res.score > *s) {
                    best = Some((res.score, bi, handle));
                }
            }
            best.map(|(_, bi, h)| (bi, h))
        }
        None => {
            for (bi, brep) in breps.into_iter().enumerate() {
                let n = match kind {
                    EntityKind::Face => brep.faces.len(),
                    EntityKind::Edge => brep.edges.len(),
                    EntityKind::Vertex => brep.vertices.len(),
                };
                for i in 0..n {
                    let (r, handle) = match kind {
                        EntityKind::Face => {
                            let id = FaceId(i as u32);
                            (TopoRef::from_face(brep, id), EntityHandle::Face(id))
                        }
                        EntityKind::Edge => {
                            let id = EdgeId(i as u32);
                            (TopoRef::from_edge(brep, id), EntityHandle::Edge(id))
                        }
                        EntityKind::Vertex => {
                            let id = VertexId(i as u32);
                            (TopoRef::from_vertex(brep, id), EntityHandle::Vertex(id))
                        }
                    };
                    if r.id_hash == eref.id_hash {
                        return Some((bi, handle));
                    }
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fielddraft_protocol::{FeatureId, Transform};

    fn id(n: u32) -> FeatureId {
        FeatureId::from_u32(n)
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
    fn parses_and_roundtrips_sketch_line() {
        let q = parse_query("qSketchLine(7, 42)").unwrap();
        assert_eq!(q, QueryExpr::SketchLine(id(7), 42));
        // Display is the canonical wire form and must re-parse.
        assert_eq!(q.to_string(), "qSketchLine(7, 42)");
        assert_eq!(parse_query(&q.to_string()).unwrap(), q);
    }

    #[test]
    fn parses_and_roundtrips_sketch_point() {
        let q = parse_query("qSketchPoint(7, 1)").unwrap();
        assert_eq!(q, QueryExpr::SketchPoint(id(7), 1));
        assert_eq!(q.to_string(), "qSketchPoint(7, 1)");
        assert_eq!(parse_query(&q.to_string()).unwrap(), q);
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
        // Two args are the role form now — a non-role second arg is an
        // invalid role, not a generic arity error.
        let err = parse_query("qCreatedBy(1, 2)").unwrap_err();
        assert_eq!(err, QueryParseError::InvalidRole("2".to_string()));
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
            "qFace(12345)",
            "qEdge(67890)",
            "qVertex(42)",
        ] {
            let parsed = parse_query(raw).unwrap();
            assert_eq!(parsed.to_string(), raw);
        }
    }

    // ─── id_hash-addressed element queries (parse) ─────────────

    #[test]
    fn parses_face_with_id_hash() {
        assert_eq!(parse_query("qFace(12345)").unwrap(), QueryExpr::Face(12345.into()));
    }

    #[test]
    fn parses_edge_with_id_hash() {
        assert_eq!(parse_query("qEdge(12345)").unwrap(), QueryExpr::Edge(12345.into()));
    }

    #[test]
    fn parses_vertex_with_id_hash() {
        assert_eq!(
            parse_query("qVertex(12345)").unwrap(),
            QueryExpr::Vertex(12345.into())
        );
    }

    #[test]
    fn parses_id_hash_beyond_i64_range() {
        // A real id_hash is a full u64 FNV-1a value and routinely exceeds
        // i64::MAX — it must parse as a u64, not silently fail.
        let big = u64::MAX;
        assert_eq!(
            parse_query(&format!("qFace({big})")).unwrap(),
            QueryExpr::Face(big.into())
        );
    }

    #[test]
    fn rejects_non_numeric_id_hash() {
        let err = parse_query("qFace(abc)").unwrap_err();
        assert!(matches!(err, QueryParseError::InvalidIdHash(_)));
    }

    #[test]
    fn rejects_wrong_arity_for_face() {
        let err = parse_query("qFace(1, 2)").unwrap_err();
        assert!(matches!(err, QueryParseError::MalformedArguments(_)));
    }

    // ─── qCreatedBy(id, ROLE) — parse ─────────────────────────

    #[test]
    fn parses_created_by_with_role() {
        for (raw, role) in [
            ("qCreatedBy(3, WALL)", FaceRole::Wall),
            ("qCreatedBy(3, CAP_TOP)", FaceRole::CapTop),
            ("qCreatedBy(3, CAP_BOTTOM)", FaceRole::CapBottom),
            ("qCreatedBy(3, INNER_WALL)", FaceRole::InnerWall),
            ("qCreatedBy(3, INNER_CAP_TOP)", FaceRole::InnerCapTop),
            ("qCreatedBy(3, INNER_CAP_BOTTOM)", FaceRole::InnerCapBottom),
            ("qCreatedBy(3, BLEND)", FaceRole::Blend),
        ] {
            let q = parse_query(raw).unwrap();
            assert_eq!(q, QueryExpr::CreatedByRole(id(3), role), "{raw}");
            // Display is the canonical wire form and must re-parse.
            assert_eq!(q.to_string(), raw);
        }
    }

    #[test]
    fn rejects_unknown_role_token() {
        let err = parse_query("qCreatedBy(3, TOP_CAP)").unwrap_err();
        assert_eq!(err, QueryParseError::InvalidRole("TOP_CAP".to_string()));
    }

    #[test]
    fn rejects_three_args_for_created_by() {
        let err = parse_query("qCreatedBy(3, CAP_TOP, WALL)").unwrap_err();
        assert!(matches!(err, QueryParseError::MalformedArguments(_)));
    }

    // ─── resolve_query_topo (element-level, S3) ───────────────

    use crate::brep::naming::{EntityKind, TopoRef};
    use crate::brep::{BRep, Edge, EdgeGeometry, Face, Loop, SurfaceGeometry};
    use crate::csg::BranchId;
    use fielddraft_protocol::HalfSpace;

    /// A trivial analytic plane face on `branch`, with an empty outer
    /// loop (a fully closed analytic surface — enough for naming, which
    /// only consults `branch_id` + the face centroid).
    fn plane_face(branch: BranchId) -> Face {
        Face {
            branch_id: branch,
            surface: SurfaceGeometry::Analytic {
                surface: HalfSpace::Plane {
                    normal: [0.0, 0.0, 1.0],
                    offset: 0.0,
                },
                transform: Transform::IDENTITY,
            },
            outer: Loop::default(),
            inner: Vec::new(),
        }
    }

    /// Build a B-Rep with the given per-feature face counts plus a few
    /// edges, exercising distinct branch ids. `face_feats[i]` faces are
    /// minted for feature id `i+1`; the edges connect the listed
    /// feature pairs.
    fn synthetic_brep(
        face_feats: &[(u32, usize)],
        edge_pairs: &[(u32, u32)],
    ) -> BRep {
        let mut brep = BRep::new();
        for &(feat, count) in face_feats {
            for sub in 0..count {
                let branch =
                    BranchId::from_feature_sub(&FeatureId::from_u32(feat), sub as u32);
                brep.add_face(plane_face(branch));
            }
        }
        for (k, &(a, b)) in edge_pairs.iter().enumerate() {
            let ba = BranchId::from_feature(&FeatureId::from_u32(a));
            let bb = BranchId::from_feature(&FeatureId::from_u32(b));
            // Distinct geometry per edge so spatial disambiguation gives
            // each a unique id even when signatures collide.
            let y = k as f64;
            brep.add_edge(Edge::new(
                EdgeGeometry::Polyline(vec![[0.0, y, 0.0], [1.0, y, 0.0]]),
                ba,
                bb,
                None,
                None,
            ));
        }
        brep
    }

    fn is_sorted_by_hash(refs: &[TopoRef]) -> bool {
        refs.windows(2).all(|w| w[0].id_hash <= w[1].id_hash)
    }

    /// A real woven cube B-Rep (6 faces, 12 edges, 8 vertices) at the given
    /// half-extent — mirrors the naming.rs / lib.rs `box_kernel` helpers so
    /// the id_hash-stability test is exercised against actual branch-derived
    /// ids, not the synthetic plane faces above.
    fn cube_brep(half: f32) -> BRep {
        use crate::brep::octree::Octree;
        use crate::brep::weaving::weave;
        use crate::bvh::BoundedCsg;
        use crate::csg::CsgNode;
        use fielddraft_protocol::Primitive;

        let csg = CsgNode::Leaf {
            id: FeatureId::from_u32(1),
            primitive: Primitive::Box {
                center: [0.0, 0.0, 0.0],
                half_extents: [half, half, half],
            },
        };
        let body = BoundedCsg::from_csg(&csg);
        let octree = Octree::build(&body, 0.25);
        weave(&body, &octree)
    }

    #[test]
    fn resolve_created_by_returns_faces_of_feature() {
        // Feature 5 owns 6 faces (a box), feature 6 owns 1.
        let brep = synthetic_brep(&[(5, 6), (6, 1)], &[]);
        let refs = resolve_query_topo(&[brep], &QueryExpr::CreatedBy(id(5)));
        assert_eq!(refs.len(), 6, "all six faces of feature 5");
        assert!(refs.iter().all(|r| r.kind == EntityKind::Face));
        assert!(is_sorted_by_hash(&refs), "sorted by id_hash");
    }

    #[test]
    fn resolve_face_of_matches_created_by_set() {
        let brep = synthetic_brep(&[(5, 6), (6, 1)], &[]);
        let created = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::CreatedBy(id(5)));
        let faces = resolve_query_topo(&[brep], &QueryExpr::FaceOf(id(5)));
        assert_eq!(created, faces, "FaceOf == CreatedBy at the face level today");
    }

    #[test]
    fn resolve_created_by_nonmatching_feature_is_empty() {
        let brep = synthetic_brep(&[(5, 6)], &[]);
        let refs = resolve_query_topo(&[brep], &QueryExpr::CreatedBy(id(999)));
        assert!(refs.is_empty(), "no faces own feature 999");
    }

    #[test]
    fn resolve_edge_between_returns_the_ab_edges() {
        // One 1–2 edge, one 1–3 edge, one 2–3 edge.
        let brep = synthetic_brep(&[(1, 1), (2, 1), (3, 1)], &[(1, 2), (1, 3), (2, 3)]);
        let refs = resolve_query_topo(&[brep], &QueryExpr::EdgeBetween(id(1), id(2)));
        assert_eq!(refs.len(), 1, "exactly the 1–2 edge");
        assert_eq!(refs[0].kind, EntityKind::Edge);
    }

    #[test]
    fn resolve_edge_between_is_unordered() {
        let brep = synthetic_brep(&[(1, 1), (2, 1)], &[(1, 2)]);
        let ab = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::EdgeBetween(id(1), id(2)));
        let ba = resolve_query_topo(&[brep], &QueryExpr::EdgeBetween(id(2), id(1)));
        assert_eq!(ab, ba, "{{a,b}} edge match is order-independent");
        assert_eq!(ab.len(), 1);
    }

    #[test]
    fn resolve_edge_between_nonmatching_pair_is_empty() {
        let brep = synthetic_brep(&[(1, 1), (2, 1)], &[(1, 2)]);
        let refs = resolve_query_topo(&[brep], &QueryExpr::EdgeBetween(id(1), id(7)));
        assert!(refs.is_empty());
    }

    #[test]
    fn resolve_region_queries_return_empty() {
        let brep = synthetic_brep(&[(5, 6)], &[]);
        for expr in [
            QueryExpr::AllRegions(id(5)),
            QueryExpr::Region(id(5), 0),
            QueryExpr::Regions(id(5), vec![0, 1]),
        ] {
            assert!(
                resolve_query_topo(std::slice::from_ref(&brep), &expr).is_empty(),
                "region queries resolve against sketch data, not the solid"
            );
        }
    }

    #[test]
    fn resolve_spans_multiple_breps() {
        let a = synthetic_brep(&[(5, 3)], &[]);
        let b = synthetic_brep(&[(5, 2)], &[]);
        let refs = resolve_query_topo(&[a, b], &QueryExpr::CreatedBy(id(5)));
        assert_eq!(refs.len(), 5, "faces from both bodies' B-Reps");
    }

    #[test]
    fn resolve_is_deterministic() {
        let brep = synthetic_brep(&[(5, 6), (6, 2)], &[(5, 6)]);
        let first = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::CreatedBy(id(5)));
        let second = resolve_query_topo(&[brep], &QueryExpr::CreatedBy(id(5)));
        assert_eq!(first, second, "two calls give identical output");
    }

    // ─── qCreatedBy(id, ROLE) — resolve ───────────────────────

    /// A face with the procedural blend-carrier surface (what a fillet
    /// mints, at sub 0 of the fillet feature's id).
    fn blend_face(branch: BranchId) -> Face {
        let wall = HalfSpace::Plane { normal: [0.0, 0.0, 1.0], offset: 0.0 };
        Face {
            branch_id: branch,
            surface: SurfaceGeometry::Analytic {
                surface: HalfSpace::Blend {
                    a: Box::new(wall.clone()),
                    a_transform: Transform::IDENTITY,
                    a_flip: false,
                    b: Box::new(wall),
                    b_transform: Transform::IDENTITY,
                    b_flip: false,
                    radius: 1.0,
                    chamfer: false,
                    is_difference: false,
                },
                transform: Transform::IDENTITY,
            },
            outer: Loop::default(),
            inner: Vec::new(),
        }
    }

    /// An extrude-shaped feature 7: two wall groups (subs 0, 1), the two
    /// caps, one shell inner wall — plus a blend face of fillet feature 9.
    fn role_brep() -> BRep {
        let f7 = FeatureId::from_u32(7);
        let f9 = FeatureId::from_u32(9);
        let mut brep = BRep::new();
        brep.add_face(plane_face(BranchId::from_feature_sub(&f7, 0)));
        brep.add_face(plane_face(BranchId::from_feature_sub(&f7, 1)));
        brep.add_face(plane_face(BranchId::from_feature_sub(&f7, BranchId::EXTRUDE_CAP_TOP_SUB)));
        brep.add_face(plane_face(BranchId::from_feature_sub(
            &f7,
            BranchId::EXTRUDE_CAP_BOTTOM_SUB,
        )));
        brep.add_face(plane_face(BranchId::from_feature_sub(&f7, 0).inner_wall()));
        // The hollow's inner floor: the cap sub WITH the inner-wall flag.
        brep.add_face(plane_face(
            BranchId::from_feature_sub(&f7, BranchId::EXTRUDE_CAP_BOTTOM_SUB).inner_wall(),
        ));
        brep.add_face(blend_face(BranchId::from_feature_sub(&f9, 0)));
        brep
    }

    #[test]
    fn resolve_created_by_role_filters_by_role() {
        let brep = role_brep();
        let f7 = id(7);
        for (role, want) in [
            (FaceRole::Wall, 2),
            (FaceRole::CapTop, 1),
            (FaceRole::CapBottom, 1),
            (FaceRole::InnerWall, 1),
            // The inner floor must NOT read as a plain inner wall — that is
            // exactly what makes the hollow's floor edge addressable.
            (FaceRole::InnerCapBottom, 1),
            (FaceRole::InnerCapTop, 0),
            (FaceRole::Blend, 0), // feature 7 has no blend face
        ] {
            let refs = resolve_query_topo(
                std::slice::from_ref(&brep),
                &QueryExpr::CreatedByRole(f7.clone(), role),
            );
            assert_eq!(refs.len(), want, "role {role}");
            assert!(refs.iter().all(|r| r.kind == EntityKind::Face));
        }
    }

    #[test]
    fn resolve_created_by_role_blend_names_the_fillet_face() {
        let brep = role_brep();
        let refs = resolve_query_topo(
            std::slice::from_ref(&brep),
            &QueryExpr::CreatedByRole(id(9), FaceRole::Blend),
        );
        assert_eq!(refs.len(), 1, "exactly the fillet's blend carrier");
        // The blend face is minted at sub 0 — WITHOUT the surface check it
        // would masquerade as a wall. The role classifier must not.
        let walls = resolve_query_topo(
            std::slice::from_ref(&brep),
            &QueryExpr::CreatedByRole(id(9), FaceRole::Wall),
        );
        assert!(walls.is_empty(), "a blend carrier is not a wall");
    }

    #[test]
    fn resolve_created_by_role_union_matches_created_by() {
        // The roles partition qCreatedBy(f)'s face set.
        let brep = role_brep();
        let all = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::CreatedBy(id(7)));
        let by_role: usize = [
            FaceRole::Wall,
            FaceRole::CapTop,
            FaceRole::CapBottom,
            FaceRole::InnerWall,
            FaceRole::InnerCapTop,
            FaceRole::InnerCapBottom,
            FaceRole::Blend,
        ]
        .iter()
        .map(|&r| {
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::CreatedByRole(id(7), r))
                .len()
        })
        .sum();
        assert_eq!(all.len(), by_role, "roles partition the CreatedBy set");
    }

    // ─── qWall / qIntersection / qAdjacentTo (Schritt 3) ──────

    #[test]
    fn parses_and_roundtrips_wall() {
        let q = parse_query("qWall(3, 7)").unwrap();
        assert_eq!(q, QueryExpr::Wall(id(3), 7, false));
        assert_eq!(q.to_string(), "qWall(3, 7)");
    }

    #[test]
    fn parses_and_roundtrips_inner_wall() {
        // The shell counterpart of the same wall — a DIFFERENT face of the
        // same sweep, which an "extrude up to the inner wall" needs to name.
        let q = parse_query("qWall(3, 7, INNER)").unwrap();
        assert_eq!(q, QueryExpr::Wall(id(3), 7, true));
        assert_eq!(q.to_string(), "qWall(3, 7, INNER)");
        assert!(matches!(
            parse_query("qWall(3, 7, OUTER)"),
            Err(QueryParseError::InvalidRole(_))
        ));
    }

    #[test]
    fn resolve_wall_inner_picks_the_hollow_side() {
        // Same entity ordinal, two faces: the wall and its inward offset. The
        // ordinal rides in the sub-id with the inner-wall flag on top, so the
        // resolver has to mask the flag off before comparing.
        let f7 = FeatureId::from_u32(7);
        let mut brep = BRep::new();
        brep.add_face(plane_face(BranchId::from_feature_sub(&f7, 3)));
        brep.add_face(plane_face(BranchId::from_feature_sub(&f7, 3).inner_wall()));
        let outer = resolve_query_topo(
            std::slice::from_ref(&brep),
            &QueryExpr::Wall(id(7), 3, false),
        );
        let inner = resolve_query_topo(
            std::slice::from_ref(&brep),
            &QueryExpr::Wall(id(7), 3, true),
        );
        assert_eq!(outer.len(), 1);
        assert_eq!(inner.len(), 1);
        assert_ne!(outer[0].id_hash, inner[0].id_hash, "two distinct faces");
    }

    #[test]
    fn parses_and_roundtrips_nested_composition() {
        // Top-level splitting must NOT break on the nested commas.
        let raw = "qIntersection(qCreatedBy(2, WALL), qAdjacentTo(qWall(2, 3), qCreatedBy(9, BLEND)))";
        let q = parse_query(raw).unwrap();
        assert_eq!(
            q,
            QueryExpr::Intersection(
                Box::new(QueryExpr::CreatedByRole(id(2), FaceRole::Wall)),
                Box::new(QueryExpr::AdjacentTo(
                    Box::new(QueryExpr::Wall(id(2), 3, false)),
                    Box::new(QueryExpr::CreatedByRole(id(9), FaceRole::Blend)),
                )),
            )
        );
        assert_eq!(q.to_string(), raw, "Display is the canonical wire form");
        assert_eq!(parse_query(&q.to_string()).unwrap(), q);
    }

    #[test]
    fn rejects_malformed_composition() {
        assert!(matches!(
            parse_query("qIntersection(qCreatedBy(1))"),
            Err(QueryParseError::MalformedArguments(_))
        ));
        assert!(parse_query("qAdjacentTo(qBogus(1), qCreatedBy(2))").is_err());
    }

    /// A three-face body with shared edges: wall A (feature 7, sub 3),
    /// wall B (feature 7, sub 6) and a blend face (feature 9). Edge 0
    /// joins wall A ↔ blend, edge 1 joins wall A ↔ wall B — so only
    /// wall A is blend-adjacent.
    fn adjacency_brep() -> BRep {
        let f7 = FeatureId::from_u32(7);
        let f9 = FeatureId::from_u32(9);
        let mut brep = BRep::new();
        let e_ab = brep.add_edge(Edge::new(
            EdgeGeometry::Polyline(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]),
            BranchId::from_feature_sub(&f7, 3),
            BranchId::from_feature_sub(&f9, 0),
            None,
            None,
        ));
        let e_aw = brep.add_edge(Edge::new(
            EdgeGeometry::Polyline(vec![[0.0, 1.0, 0.0], [1.0, 1.0, 0.0]]),
            BranchId::from_feature_sub(&f7, 3),
            BranchId::from_feature_sub(&f7, 6),
            None,
            None,
        ));
        let mut wall_a = plane_face(BranchId::from_feature_sub(&f7, 3));
        wall_a.outer = Loop::new(vec![(e_ab, false), (e_aw, false)]);
        let mut wall_b = plane_face(BranchId::from_feature_sub(&f7, 6));
        wall_b.outer = Loop::new(vec![(e_aw, true)]);
        let mut blend = blend_face(BranchId::from_feature_sub(&f9, 0));
        blend.outer = Loop::new(vec![(e_ab, true)]);
        brep.add_face(wall_a);
        brep.add_face(wall_b);
        brep.add_face(blend);
        brep
    }

    #[test]
    fn resolve_wall_picks_by_stable_entity_sub() {
        let brep = adjacency_brep();
        let refs =
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Wall(id(7), 3, false));
        assert_eq!(refs.len(), 1, "exactly the sub-3 wall");
        assert!(resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Wall(id(7), 5, false))
            .is_empty());
        // A blend face at sub 0 must NOT satisfy qWall(9, 0).
        assert!(resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Wall(id(9), 0, false))
            .is_empty());
    }

    #[test]
    fn resolve_intersection_intersects_by_identity() {
        let brep = adjacency_brep();
        let q = QueryExpr::Intersection(
            Box::new(QueryExpr::CreatedByRole(id(7), FaceRole::Wall)),
            Box::new(QueryExpr::Wall(id(7), 3, false)),
        );
        let refs = resolve_query_topo(std::slice::from_ref(&brep), &q);
        assert_eq!(refs.len(), 1, "both walls ∩ the sub-3 wall = one face");
        let disjoint = QueryExpr::Intersection(
            Box::new(QueryExpr::Wall(id(7), 3, false)),
            Box::new(QueryExpr::Wall(id(7), 6, false)),
        );
        assert!(resolve_query_topo(std::slice::from_ref(&brep), &disjoint).is_empty());
    }

    #[test]
    fn resolve_adjacent_to_picks_the_blend_neighbour() {
        // THE Kabelbox disambiguation shape: of the walls feature 7
        // swept, pick the one sharing an edge with the fillet's blend.
        let brep = adjacency_brep();
        let q = QueryExpr::AdjacentTo(
            Box::new(QueryExpr::CreatedByRole(id(7), FaceRole::Wall)),
            Box::new(QueryExpr::CreatedByRole(id(9), FaceRole::Blend)),
        );
        let refs = resolve_query_topo(std::slice::from_ref(&brep), &q);
        assert_eq!(refs.len(), 1, "only wall A touches the blend");
        let want = TopoRef::from_face(&brep, crate::brep::FaceId(0));
        assert_eq!(refs[0].id_hash, want.id_hash, "and it is wall A");
    }

    // ─── id_hash-addressed element queries (resolve) ──────────

    #[test]
    fn resolve_face_by_id_hash_picks_exactly_that_face() {
        let brep = cube_brep(10.0);
        assert_eq!(brep.face_count(), 6, "a box has 6 faces");

        // Capture one specific face's id_hash, then address it directly.
        let pick = crate::brep::FaceId(2);
        let want = TopoRef::from_face(&brep, pick);

        let refs = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Face(want.id_hash.into()));
        assert_eq!(refs.len(), 1, "id_hash addresses exactly one face, not 6");
        assert_eq!(refs[0].kind, EntityKind::Face);
        assert_eq!(refs[0].id_hash, want.id_hash);
        assert_eq!(refs[0], want, "the resolved face is the one we addressed");
    }

    #[test]
    fn resolve_edge_by_id_hash_picks_exactly_that_edge() {
        let brep = cube_brep(10.0);
        assert_eq!(brep.edge_count(), 12);
        let want = TopoRef::from_edge(&brep, crate::brep::EdgeId(5));
        let refs = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Edge(want.id_hash.into()));
        assert_eq!(refs.len(), 1, "id_hash addresses exactly one edge");
        assert_eq!(refs[0].kind, EntityKind::Edge);
        assert_eq!(refs[0], want);
    }

    #[test]
    fn resolve_vertex_by_id_hash_picks_exactly_that_vertex() {
        let brep = cube_brep(10.0);
        assert_eq!(brep.vertex_count(), 8);
        let want = TopoRef::from_vertex(&brep, crate::brep::VertexId(3));
        let refs =
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Vertex(want.id_hash.into()));
        assert_eq!(refs.len(), 1, "id_hash addresses exactly one vertex");
        assert_eq!(refs[0].kind, EntityKind::Vertex);
        assert_eq!(refs[0], want);
    }

    #[test]
    fn resolve_face_id_hash_survives_resize() {
        // STABILITY: an id_hash is branch-derived, not geometry-derived, so a
        // stored qFace(h) from a 10mm box re-resolves to the corresponding
        // face of a 40mm box (a pure parameter change is topology-stable).
        let small = cube_brep(10.0);
        let big = cube_brep(40.0);

        let pick = crate::brep::FaceId(2);
        let want = TopoRef::from_face(&small, pick);

        let refs = resolve_query_topo(std::slice::from_ref(&big), &QueryExpr::Face(want.id_hash.into()));
        assert_eq!(
            refs.len(),
            1,
            "the 10mm-box face id_hash still addresses exactly one face on the 40mm box"
        );
        assert_eq!(refs[0].id_hash, want.id_hash, "id_hash is regeneration-stable");
        // The matched face on the big box has the same branch as the small one.
        let big_branch = big.face(crate::brep::FaceId(2)).branch_id;
        let _ = big_branch; // (ordering may differ; id_hash equality is the contract)
    }

    // ─── fingerprint-carrying element refs (N2/N3) ──────────

    #[test]
    fn element_query_long_form_round_trips() {
        let brep = cube_brep(10.0);
        for q in [
            element_query(&TopoRef::from_face(&brep, crate::brep::FaceId(2))),
            element_query(&TopoRef::from_edge(&brep, crate::brep::EdgeId(5))),
            element_query(&TopoRef::from_vertex(&brep, crate::brep::VertexId(3))),
        ] {
            let s = q.to_string();
            let parsed = parse_query(&s).unwrap_or_else(|e| panic!("parse `{s}`: {e}"));
            assert_eq!(parsed, q, "Display ↔ parse_query must round-trip `{s}`");
        }
    }

    #[test]
    fn short_form_stays_hash_only() {
        let q = parse_query("qFace(12345)").unwrap();
        assert_eq!(q, QueryExpr::Face(ElementRef::hash_only(12345)));
        assert_eq!(q.to_string(), "qFace(12345)");
    }

    #[test]
    fn malformed_fingerprint_block_is_rejected() {
        for bad in [
            "qFace(12345 @ 1 2 3)",       // wrong arity (3)
            "qFace(12345 @ 1 2 3 4 5)",   // wrong arity (5)
            "qEdge(12345 @ a b c d)",     // non-numeric
        ] {
            assert!(
                matches!(parse_query(bad), Err(QueryParseError::InvalidFingerprint(_))),
                "`{bad}` must be rejected"
            );
        }
    }

    #[test]
    fn fingerprint_ref_heals_across_hash_loss() {
        // The stored hash belongs to no element of the rebuilt B-Rep (a
        // bogus hash stands in for a topology change that renamed the
        // element), but the fingerprint pins the geometry — the reference
        // heals to the fingerprint-nearest element instead of vanishing.
        let brep = cube_brep(10.0);
        let want = TopoRef::from_face(&brep, crate::brep::FaceId(2));
        let eref = ElementRef {
            id_hash: 0xDEAD_BEEF_DEAD_BEEFu64,
            fingerprint: Some(want.fingerprint),
        };
        let refs = resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Face(eref));
        assert_eq!(refs.len(), 1, "the fingerprint must heal the lost hash");
        assert_eq!(refs[0], want, "healing binds to the fingerprint's face");
    }

    #[test]
    fn fingerprint_ref_without_match_is_empty() {
        // Bogus hash AND a fingerprint far outside the body: nothing to
        // heal towards — the reference reports unresolved (no arbitrary pick).
        let brep = cube_brep(10.0);
        let eref = ElementRef {
            id_hash: 0xDEAD_BEEF_DEAD_BEEFu64,
            fingerprint: Some(crate::brep::naming::Fingerprint {
                centroid: [1e6, 1e6, 1e6],
                size: 1e-3,
                tangent: [0.0; 3],
            }),
        };
        assert!(
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Face(eref)).is_empty(),
            "a reference that matches nothing must not bind"
        );
    }

    #[test]
    fn resolve_nonexistent_id_hash_is_empty() {
        let brep = cube_brep(10.0);
        // A hash that belongs to no element of any kind.
        let bogus = 0xDEAD_BEEF_DEAD_BEEFu64;
        assert!(
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Face(bogus.into())).is_empty()
        );
        assert!(
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Edge(bogus.into())).is_empty()
        );
        assert!(
            resolve_query_topo(std::slice::from_ref(&brep), &QueryExpr::Vertex(bogus.into())).is_empty()
        );
    }
}
