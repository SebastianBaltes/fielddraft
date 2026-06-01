//! FieldDraft wire protocol — shared types for kernel ↔ frontend.
//!
//! This crate stays `no_std`-friendly-ish and avoids any wasm-bindgen deps,
//! so the same types can be consumed from the kernel crate and (later) a
//! JSON-schema export for the TypeScript side.

use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;

use serde::de::{self, Deserializer, Visitor};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

pub mod mc_tables;
pub mod mesh_sdf;
pub mod sketch2d;
pub mod voxel_repair;
pub use mesh_sdf::{BakedVolume, MeshSdf};
pub use sketch2d::{Aabb2, Sketch2d, Sketch2dPrimitive, SketchSegment};

/// Current on-disk / on-wire schema version for the project document.
pub const SCHEMA_VERSION: u32 = 2;

/// Hierarchical, typed identifier for a feature or one of its sub-nodes.
///
/// The K4.5 "Feature-as-Function" refactor replaces the flat `u32` ids
/// from K1/K2/K4 with a tree-structured id so sub-operations inside a
/// feature body can carve out their own stable namespace. Concretely:
///
/// * `parent` is an optional ancestor id — `None` for top-level
///   feature entries in the document, `Some` for sub-nodes created
///   inside a feature body.
/// * `local` is the short per-level name stored as [`SmolStr`] so
///   the common flat-numeric form (`"1"`, `"42"`) stays inline-
///   allocated without hitting the heap.
///
/// Pretty-printed as a dotted path (`extrude_3.op`, `fillet_7.mask_0`)
/// via the [`fmt::Display`] impl. Parseable back via [`FromStr`] with
/// the mirror rules.
///
#[derive(Debug, Clone, Eq, PartialEq, Hash, Default)]
pub struct FeatureId {
    pub parent: Option<Box<FeatureId>>,
    pub local: SmolStr,
}

impl FeatureId {
    /// Build a top-level id from a textual local name.
    pub fn root(local: impl Into<SmolStr>) -> Self {
        Self {
            parent: None,
            local: local.into(),
        }
    }

    /// Build a top-level id from a flat numeric counter.
    pub fn from_u32(n: u32) -> Self {
        Self::root(n.to_string())
    }

    /// Build a child id underneath this one. The new id keeps the
    /// whole ancestor chain so `Display` can emit the full path.
    pub fn child(&self, local: impl Into<SmolStr>) -> Self {
        Self {
            parent: Some(Box::new(self.clone())),
            local: local.into(),
        }
    }

    /// Return the parent id, if any.
    pub fn parent(&self) -> Option<&FeatureId> {
        self.parent.as_deref()
    }

    /// `true` when this id has no parent and the local part parses
    /// cleanly as a `u32`.
    pub fn is_flat_numeric(&self) -> bool {
        self.parent.is_none() && self.local.parse::<u32>().is_ok()
    }

    /// Collapse the id back to a `u32`, assuming it's flat-numeric.
    /// Returns `None` for hierarchical or non-numeric ids.
    pub fn as_u32_if_flat(&self) -> Option<u32> {
        if self.parent.is_some() {
            return None;
        }
        self.local.parse::<u32>().ok()
    }

    /// Collapse to a u32, falling back to the root ancestor's numeric
    /// id for hierarchical ids like `2.0` (hole child).
    pub fn as_u32_or_root(&self) -> Option<u32> {
        self.as_u32_if_flat()
            .or_else(|| self.root_ancestor().as_u32_if_flat())
    }

    /// Walk up the parent chain and return the root-most ancestor.
    pub fn root_ancestor(&self) -> &FeatureId {
        let mut cur = self;
        while let Some(p) = &cur.parent {
            cur = p;
        }
        cur
    }
}

impl fmt::Display for FeatureId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(parent) = &self.parent {
            write!(f, "{parent}.{}", self.local)
        } else {
            f.write_str(&self.local)
        }
    }
}

/// Parse error for [`FeatureId`]. A future hierarchical slice may
/// grow this with richer variants (empty segments, forbidden chars).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureIdParseError {
    pub message: &'static str,
}

impl fmt::Display for FeatureIdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for FeatureIdParseError {}

impl From<ParseIntError> for FeatureIdParseError {
    fn from(_: ParseIntError) -> Self {
        Self {
            message: "invalid numeric feature id",
        }
    }
}

impl FromStr for FeatureId {
    type Err = FeatureIdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(FeatureIdParseError {
                message: "empty feature id",
            });
        }
        // Split at the *last* dot so parent chains parse left-
        // to-right but build right-to-left, matching the
        // [`Display`] shape `"grandparent.parent.local"`.
        match s.rsplit_once('.') {
            None => Ok(FeatureId::root(s)),
            Some((head, tail)) => {
                if tail.is_empty() {
                    return Err(FeatureIdParseError {
                        message: "empty local segment in feature id",
                    });
                }
                let parent = FeatureId::from_str(head)?;
                Ok(FeatureId {
                    parent: Some(Box::new(parent)),
                    local: SmolStr::from(tail),
                })
            }
        }
    }
}

impl Serialize for FeatureId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if let Some(n) = self.as_u32_if_flat() {
            serializer.serialize_u32(n)
        } else {
            serializer.collect_str(self)
        }
    }
}

impl<'de> Deserialize<'de> for FeatureId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FeatureIdVisitor;

        impl<'de> Visitor<'de> for FeatureIdVisitor {
            type Value = FeatureId;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a feature id: integer or dotted string")
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<FeatureId, E> {
                let v = u32::try_from(v).map_err(|_| E::custom("feature id out of u32 range"))?;
                Ok(FeatureId::from_u32(v))
            }

            fn visit_u32<E: de::Error>(self, v: u32) -> Result<FeatureId, E> {
                Ok(FeatureId::from_u32(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<FeatureId, E> {
                if v < 0 {
                    return Err(E::custom("negative feature id"));
                }
                let v = u32::try_from(v).map_err(|_| E::custom("feature id out of u32 range"))?;
                Ok(FeatureId::from_u32(v))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<FeatureId, E> {
                FeatureId::from_str(v).map_err(E::custom)
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<FeatureId, E> {
                FeatureId::from_str(&v).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(FeatureIdVisitor)
    }
}

/// A single SDF primitive shape. The primitive itself carries no
/// boolean operation — that lives on its enclosing `Feature`.
///
/// K7.3 adds the `Extrude` variant so the extrude feature can
/// lower into a real sketch-profile volume rather than a
/// rectangular Box stand-in. The sketch payload is a full
/// [`Sketch2d`] tree (including unions / differences), and
/// `half_depth_z` plus `center` define the world-space slab
/// the profile sweeps along the local Z axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Primitive {
    Box { center: [f32; 3], half_extents: [f32; 3] },
    Sphere { center: [f32; 3], radius: f32 },
    Cylinder { center: [f32; 3], radius: f32, half_height: f32 },
    /// K7.3: sketch-profile extrusion. The `sketch` field carries
    /// the 2D region that gets swept; `half_depth_z` is half the
    /// total extrude height along the local Z axis; `center`
    /// positions the resulting slab in world space (the extrude
    /// plane is at `center[2]`).
    Extrude {
        sketch: Sketch2d,
        half_depth_z: f32,
        center: [f32; 3],
    },
    /// Torus centred at the origin, major radius R (centre of
    /// tube to axis), minor radius r (tube cross-section).
    /// SDF: `length(vec2(length(p.xz) - R, p.y)) - r`.
    Torus {
        center: [f32; 3],
        major_radius: f32,
        minor_radius: f32,
    },
    /// Regular hexagonal prism along the Y axis. `apothem` is the
    /// center-to-flat distance (= circumradius · cos(30°)). Used
    /// for hex bolts, nuts, Allen keys.
    HexPrism {
        apothem: f32,
        half_height: f32,
    },
    /// Ellipsoid with independent radii along X, Y, Z axes.
    /// SDF is approximate (coordinate scaling + sphere) but
    /// accurate enough for DC meshing.
    Ellipsoid {
        radii: [f32; 3],
    },
    /// Capsule (line-segment swept sphere) along the Y axis.
    /// `half_height` is the half-length of the inner segment
    /// (total height = 2 * half_height + 2 * radius).
    Capsule {
        radius: f32,
        half_height: f32,
    },
    /// Truncated cone (frustum) with bottom radius `r_bottom`,
    /// top radius `r_top`, and half-height along Y. When
    /// `r_top == 0` it's a perfect cone.
    Cone {
        r_bottom: f32,
        r_top: f32,
        half_height: f32,
    },
    /// K12: metric thread via direct helix formula. O(1) per
    /// sample — no sweep, no path discretisation. The groove
    /// is carved into a cylinder of `nominal_radius` with a
    /// helical pitch and thread depth.
    Thread {
        nominal_radius: f32,
        pitch: f32,
        depth: f32,
        half_height: f32,
    },
    /// K13.1: imported triangle mesh as a CSG-participating solid.
    /// The `MeshSdf` carries the flat triangle buffer + per-tri
    /// AABBs and provides an O(N) nearest-triangle + ray-parity
    /// SDF. Serialised inline — no external file reference.
    ImportedMesh {
        mesh: Box<MeshSdf>,
    },
    /// K11.0: Gyroid TPMS lattice. The raw implicit is
    /// `sin(x/s)cos(y/s) + sin(y/s)cos(z/s) + sin(z/s)cos(x/s)`,
    /// thickened into a solid shell via `|f| - thickness`.
    /// The field repeats infinitely — intersect with a host body
    /// to clip it to a finite region.
    Gyroid {
        cell_size: f32,
        thickness: f32,
    },
    /// K8.4: sketch-profile revolution around the local Y axis.
    /// The sketch `x` coordinate maps to the radial distance
    /// `√(x² + z²)` in 3D, its `y` stays vertical. Authoring
    /// convention: sketch primitives should live at `x ≥ 0`;
    /// primitives that cross the axis produce valid but
    /// geometrically silly results. `center` positions the axis
    /// of revolution in world space.
    Revolve {
        sketch: Sketch2d,
        center: [f32; 3],
    },
}

/// Analytic surface a primitive sub-face lies on. The B-Rep substrate
/// decomposes box/cylinder/cone primitives into their per-face
/// half-spaces (one [`crate::Primitive`] face → one `HalfSpace`) so each
/// face carries a stable sub-index and an exact surface the weaver can
/// attach to. Surfaces are expressed in the primitive's *centred* local
/// frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HalfSpace {
    Plane { normal: [f32; 3], offset: f32 },
    InfiniteCylinder { radius: f32 },
    Sphere { radius: f32 },
    Cone { half_angle: f32 },
    Torus { major_radius: f32, minor_radius: f32 },
    Gyroid { cell_size: f32, thickness: f32 },
}

impl Primitive {
    /// Numeric kind tag matching the packed-buffer / shader contract.
    pub const KIND_BOX: u32 = 0;
    pub const KIND_SPHERE: u32 = 1;
    pub const KIND_CYLINDER: u32 = 2;
    pub const KIND_EXTRUDE: u32 = 3;
    pub const KIND_REVOLVE: u32 = 4;
    pub const KIND_GYROID: u32 = 5;
    pub const KIND_TORUS: u32 = 6;
    pub const KIND_THREAD: u32 = 7;
    pub const KIND_CONE: u32 = 8;
    pub const KIND_CAPSULE: u32 = 9;
    pub const KIND_HEX_PRISM: u32 = 10;
    pub const KIND_IMPORTED_MESH: u32 = 11;
    pub const KIND_ELLIPSOID: u32 = 12;

    pub fn kind_tag(&self) -> u32 {
        match self {
            Primitive::Box { .. } => Self::KIND_BOX,
            Primitive::Sphere { .. } => Self::KIND_SPHERE,
            Primitive::Cylinder { .. } => Self::KIND_CYLINDER,
            Primitive::Extrude { .. } => Self::KIND_EXTRUDE,
            Primitive::HexPrism { .. } => Self::KIND_HEX_PRISM,
            Primitive::Capsule { .. } => Self::KIND_CAPSULE,
            Primitive::Torus { .. } => Self::KIND_TORUS,
            Primitive::Cone { .. } => Self::KIND_CONE,
            Primitive::Thread { .. } => Self::KIND_THREAD,
            Primitive::Gyroid { .. } => Self::KIND_GYROID,
            Primitive::Revolve { .. } => Self::KIND_REVOLVE,
            Primitive::ImportedMesh { .. } => Self::KIND_IMPORTED_MESH,
            Primitive::Ellipsoid { .. } => Self::KIND_ELLIPSOID,
        }
    }

    pub fn default_name(&self) -> &'static str {
        match self {
            Primitive::Box { .. } => "Box",
            Primitive::Sphere { .. } => "Sphere",
            Primitive::Cylinder { .. } => "Cylinder",
            Primitive::Extrude { .. } => "Extrude",
            Primitive::HexPrism { .. } => "Hex prism",
            Primitive::Capsule { .. } => "Capsule",
            Primitive::Torus { .. } => "Torus",
            Primitive::Cone { .. } => "Cone",
            Primitive::Thread { .. } => "Thread",
            Primitive::Gyroid { .. } => "Gyroid",
            Primitive::Revolve { .. } => "Revolve",
            Primitive::ImportedMesh { .. } => "Imported mesh",
            Primitive::Ellipsoid { .. } => "Ellipsoid",
        }
    }
}

/// Boolean operation that combines a feature with the running result.
///
/// Evaluation order is the feature list itself: each feature folds into
/// the accumulator using its op. The very first `Union` feature seeds the
/// accumulator from `+∞` (empty space), so a list of `[Union(box)]`
/// renders that single box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoolOp {
    Union,
    Difference,
    Intersect,
}

impl BoolOp {
    /// Numeric tag for the packed buffer / shader contract.
    pub const TAG_UNION: u32 = 0;
    pub const TAG_DIFFERENCE: u32 = 1;
    pub const TAG_INTERSECT: u32 = 2;

    pub fn tag(self) -> u32 {
        match self {
            BoolOp::Union => Self::TAG_UNION,
            BoolOp::Difference => Self::TAG_DIFFERENCE,
            BoolOp::Intersect => Self::TAG_INTERSECT,
        }
    }
}

// ─── K4.5.4f: typed runtime parameter types ──────────────────────
//
// Length, Angle, and Query are the first typed parameter values in
// the FieldDraft feature system. They grow the FeatureSchema derive
// beyond the plain-scalar vocabulary of K4.5.4a (f32, [f32;3], bool,
// String, BoolOp) so that upcoming Rust-native feature definitions
// (Extrude, Revolve, hole patterns, …) can describe their parameters
// in terms of physical quantities and geometric references rather
// than bare numbers and strings.
//
// The model unit for lengths is millimetres (the kernel's native
// distance scale); `Length::to_mm` resolves any unit to this scale
// so feature execute paths can always work in model space. The
// model unit for angles is radians. Conversions are lossy (f32),
// matching the rest of the kernel.
//
// `Query` is a placeholder for K4.5.7's query minimum
// (`qCreatedBy`/`qFaceOf`/`qEdgeBetween`). Step 4f only introduces
// the wire type so features can declare query-shaped parameters;
// the actual evaluation against the contributor graph lands with
// the runtime K4.5.7 work.

/// Length unit, wire-encoded as a lowercase string (`"mm"`, `"cm"`,
/// `"m"`, `"in"`). The model unit is millimetres — [`Length::to_mm`]
/// converts any unit to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LengthUnit {
    Mm,
    Cm,
    M,
    In,
}

/// A length value with an explicit unit. Wire form is
/// `{"value": 10.0, "unit": "mm"}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Length {
    pub value: f32,
    pub unit: LengthUnit,
}

impl Length {
    pub const fn mm(value: f32) -> Self {
        Self { value, unit: LengthUnit::Mm }
    }
    pub const fn cm(value: f32) -> Self {
        Self { value, unit: LengthUnit::Cm }
    }
    pub const fn m(value: f32) -> Self {
        Self { value, unit: LengthUnit::M }
    }
    pub const fn inches(value: f32) -> Self {
        Self { value, unit: LengthUnit::In }
    }

    /// Convert to the kernel's native model unit (millimetres).
    pub fn to_mm(&self) -> f32 {
        match self.unit {
            LengthUnit::Mm => self.value,
            LengthUnit::Cm => self.value * 10.0,
            LengthUnit::M => self.value * 1000.0,
            LengthUnit::In => self.value * 25.4,
        }
    }
}

/// Angle unit, wire-encoded as a lowercase string (`"deg"`, `"rad"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AngleUnit {
    Deg,
    Rad,
}

/// An angle value with an explicit unit. Wire form is
/// `{"value": 45.0, "unit": "deg"}`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Angle {
    pub value: f32,
    pub unit: AngleUnit,
}

impl Angle {
    pub const fn deg(value: f32) -> Self {
        Self { value, unit: AngleUnit::Deg }
    }
    pub const fn rad(value: f32) -> Self {
        Self { value, unit: AngleUnit::Rad }
    }

    /// Convert to the kernel's native model unit (radians).
    pub fn to_rad(&self) -> f32 {
        match self.unit {
            AngleUnit::Deg => self.value * std::f32::consts::PI / 180.0,
            AngleUnit::Rad => self.value,
        }
    }
}

/// A geometry-selection query that a feature parameter references.
///
/// Wire form is a bare string — `"qCreatedBy(box_1)"`,
/// `"qFaceOf(extrude_3, top)"`, etc. K4.5.4f only introduces the
/// type so features can declare query parameters; actual
/// evaluation against the contributor graph is K4.5.7's
/// responsibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Query {
    pub expr: String,
}

impl Query {
    pub fn new(expr: impl Into<String>) -> Self {
        Self { expr: expr.into() }
    }

    pub fn as_str(&self) -> &str {
        &self.expr
    }
}

/// A multi-query: comma-separated list of query expressions.
///
/// Wire form is `"qCreatedBy(box_1),qCreatedBy(sphere_2)"`.
/// Set semantics — no duplicates expected at the UI layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MultiQuery {
    pub expr: String,
}

impl MultiQuery {
    pub fn new(expr: impl Into<String>) -> Self {
        Self { expr: expr.into() }
    }

    pub fn as_str(&self) -> &str {
        &self.expr
    }

    pub fn queries(&self) -> Vec<&str> {
        if self.expr.is_empty() {
            Vec::new()
        } else {
            self.expr.split(',').map(|s| s.trim()).collect()
        }
    }
}

/// Affine placement of a feature in world space.
///
/// Stored as **translate + unit-quaternion rotation + per-axis scale**.
/// Non-uniform scale requires a conservative distance correction in
/// the SDF evaluator: multiply by `min(sx, sy, sz)`.
/// Quaternion order is `(x, y, z, w)`; identity is `(0, 0, 0, 1)`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translate: [f32; 3],
    pub rotation: [f32; 4],
    #[serde(deserialize_with = "deserialize_scale")]
    pub scale: [f32; 3],
}

fn deserialize_scale<'de, D: serde::Deserializer<'de>>(d: D) -> Result<[f32; 3], D::Error> {
    let v: serde_json::Value = serde::Deserialize::deserialize(d)?;
    match v {
        serde_json::Value::Number(n) => {
            let s = n.as_f64().unwrap_or(1.0) as f32;
            Ok([s, s, s])
        }
        serde_json::Value::Array(arr) if arr.len() == 3 => {
            let x = arr[0].as_f64().unwrap_or(1.0) as f32;
            let y = arr[1].as_f64().unwrap_or(1.0) as f32;
            let z = arr[2].as_f64().unwrap_or(1.0) as f32;
            Ok([x, y, z])
        }
        _ => Ok([1.0, 1.0, 1.0]),
    }
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translate: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
    };

    pub fn is_identity(&self) -> bool {
        self.translate == [0.0, 0.0, 0.0]
            && self.rotation == [0.0, 0.0, 0.0, 1.0]
            && self.scale == [1.0, 1.0, 1.0]
    }

    pub fn min_scale(&self) -> f32 {
        self.scale[0].min(self.scale[1]).min(self.scale[2])
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// One node in the feature spine. At M2 the spine is linear; later
/// milestones (Pattern, Group, Mirror) will introduce nested children
/// that share the same `Feature` envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: FeatureId,
    pub name: String,
    pub op: BoolOp,
    pub primitive: Primitive,
    /// Affine placement of the primitive in world space.
    /// `#[serde(default)]` lets us roll forward older documents that
    /// predate transforms — they load with identity placement.
    #[serde(default)]
    pub transform: Transform,
    /// K10.1: per-junction blend radius in mm. When > 0 the CSG
    /// tree builder (`CsgNode::from_features`) uses
    /// `SmoothUnion`/`SmoothDifference` instead of the hard
    /// variants for this feature's Boolean junction. Zero (the
    /// default) keeps the classic sharp edges.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub blend_radius: f32,
    /// K11.1: twist rate in radians per mm around the local Y
    /// axis. When non-zero, `CsgNode::from_features` wraps
    /// this feature's leaf in a `CsgNode::Twist` before folding
    /// it into the accumulator. Zero = no twist.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub twist_rate: f32,
    /// K11.2: bend rate in radians per mm in the XY plane along
    /// the X axis. Same domain-warp approach as twist_rate.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bend_rate: f32,
    /// K11.3: displacement depth in mm. When > 0 the CSG builder
    /// wraps the leaf in `CsgNode::Displace`. Zero = smooth surface.
    /// K10.2: chamfer size in mm. When > 0 and `blend_radius` is
    /// 0, `from_features` uses the chamfer blend formula (flat
    /// 45° cut) instead of the fillet. Mutually exclusive with
    /// `blend_radius` at the UX level; if both are set the fillet
    /// wins.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub chamfer_size: f32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub displace_depth: f32,
    /// K11.3: displacement spatial frequency (cycles/mm).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub displace_freq: f32,
    /// Shell thickness in mm. When > 0, from_features wraps the
    /// leaf in CsgNode::Shell, turning the solid into a thin
    /// wall of this thickness.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub shell_thickness: f32,
    /// Explicit body-targeting: when non-empty, the partitioner
    /// attaches this feature only to bodies that contain one of
    /// these feature IDs, instead of using AABB overlap.
    /// Used by Gyroid (infinite AABB) and future lattice prims.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_ids: Vec<FeatureId>,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// One entry in the K4.5 Feature-as-Function feature tree.
///
/// The canonical §4.6 JSON wire format for every mutation a user
/// makes to a document: a `{ type, id, params }` triple where the
/// type name picks a [`FeatureKind`] (in the kernel registry) and
/// the `params` blob carries the user's typed parameter values as
/// raw JSON until a `FeatureSchema` derive validates them.
///
/// K4.5.4b moved this struct from `kernel::features` to the
/// protocol crate so it can eventually live inside [`Document`]
/// as the authoritative serialisation form (K4.5.4c+). The kernel
/// re-exports `FeatureCall` from `crate::features::FeatureCall`
/// so existing imports keep working — the move is purely
/// structural, no behaviour changed in this slice.
///
/// [`FeatureKind`]: ../fielddraft_kernel/features/trait.FeatureKind.html
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureCall {
    /// Registry key for the feature type. `#[serde(rename = "type")]`
    /// emits the §4.6 canonical field name on the wire.
    #[serde(rename = "type")]
    pub type_name: String,
    /// Stable id of this call within the feature tree. Step 1
    /// (K4.5.1) introduced the hierarchical [`FeatureId`] that
    /// sub-operations inside a feature body can extend with
    /// child names.
    pub id: FeatureId,
    /// Typed parameter values. Held as raw `serde_json::Value` so
    /// the wire shape is schema-less until the feature's
    /// `FeatureKind::execute` parses it.
    #[serde(default)]
    pub params: serde_json::Value,
    /// K8.7: per-call suppression flag. When `true`, the
    /// regeneration loop skips this call — its geometry drops
    /// out of the CSG tree but the call itself stays in the
    /// document so the user can unsuppress it later. Skipped
    /// from serialization when `false` so existing fixtures
    /// and round-trip tests see a byte-identical wire form.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub suppressed: bool,
}

impl FeatureCall {
    /// Build a feature call from its three canonical fields.
    pub fn new(
        type_name: impl Into<String>,
        id: FeatureId,
        params: serde_json::Value,
    ) -> Self {
        Self {
            type_name: type_name.into(),
            id,
            params,
            suppressed: false,
        }
    }
}

/// Project document root.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub schema_version: u32,
    pub name: String,
    /// In-memory feature spine. Not serialized — the wire format
    /// uses `feature_calls` exclusively. On deserialize the kernel
    /// rehydrates this from `feature_calls` via `FeatureRegistry::regenerate`.
    #[serde(skip_serializing, default)]
    pub features: Vec<Feature>,
    /// Canonical wire form: each entry is a typed function call
    /// that the kernel can replay into the `features` spine.
    #[serde(default)]
    pub feature_calls: Vec<FeatureCall>,
    /// Monotonic counter that seeds the next [`FeatureId`].
    pub next_feature_id: u32,
    /// K8.10: document-level scalar variables. Keyed by
    /// user-chosen name, value in the model unit
    /// (millimetres for lengths, dimensionless otherwise).
    /// The K8.11 formula engine will teach Length fields to
    /// reference these by name; this first slice is just the
    /// persisted table. BTreeMap so the wire form is
    /// deterministically ordered across saves — snapshots
    /// diff cleanly.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub variables: std::collections::BTreeMap<String, f32>,
}

impl Default for Document {
    fn default() -> Self {
        // The empty default has no features at all so the user starts
        // with a clean slate. Tests and the smoke E2E both rely on this.
        Self {
            schema_version: SCHEMA_VERSION,
            name: "Untitled".to_string(),
            features: Vec::new(),
            feature_calls: Vec::new(),
            next_feature_id: 1,
            variables: std::collections::BTreeMap::new(),
        }
    }
}

impl Document {
    /// Allocate the next flat-numeric [`FeatureId`] and bump the
    /// counter. K4.5.1 still uses the K1 allocation scheme (`"1"`,
    /// `"2"`, `"3"` in the new typed form). Hierarchical allocation
    /// via the Feature Registry will come in a later slice.
    pub fn allocate_id(&mut self) -> FeatureId {
        let n = self.next_feature_id;
        self.next_feature_id = self.next_feature_id.saturating_add(1);
        FeatureId::from_u32(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_document_is_empty() {
        let doc = Document::default();
        assert_eq!(doc.schema_version, SCHEMA_VERSION);
        assert!(doc.features.is_empty());
        assert_eq!(doc.next_feature_id, 1);
    }

    // ─── Document.feature_calls (K4.5.4c) ──────────────────────

    #[test]
    fn default_document_has_empty_feature_calls() {
        let doc = Document::default();
        assert!(doc.feature_calls.is_empty());
    }

    #[test]
    fn document_round_trips_feature_calls_through_json() {
        // K4.5.4e: the only feature list that survives a JSON
        // round-trip is `feature_calls`. `features` is
        // `skip_serializing`, so the parsed document comes back
        // with an empty spine — the kernel bootstraps it from
        // `feature_calls` in `load_json`. At the protocol level
        // we assert only the wire-carried fields.
        let mut doc = Document::default();
        let id = doc.allocate_id();
        doc.feature_calls.push(FeatureCall::new(
            "box",
            id.clone(),
            serde_json::json!({ "half_extents": [10.0, 10.0, 10.0] }),
        ));

        let wire = serde_json::to_string(&doc).unwrap();
        let parsed: Document = serde_json::from_str(&wire).unwrap();
        assert_eq!(parsed.schema_version, doc.schema_version);
        assert_eq!(parsed.name, doc.name);
        assert_eq!(parsed.next_feature_id, doc.next_feature_id);
        assert!(parsed.features.is_empty());
        assert_eq!(parsed.feature_calls, doc.feature_calls);
        assert_eq!(parsed.feature_calls.len(), 1);
        assert_eq!(parsed.feature_calls[0].id, id);
    }

    #[test]
    fn document_json_wire_form_omits_features_field() {
        // K4.5.4e: serialize a populated document and assert the
        // wire-form JSON contains `feature_calls` but no
        // `features` key at the top level.
        let mut doc = Document::default();
        let id = doc.allocate_id();
        doc.feature_calls.push(FeatureCall::new(
            "box",
            id,
            serde_json::json!({ "half_extents": [1.0, 1.0, 1.0] }),
        ));

        let wire = serde_json::to_value(&doc).unwrap();
        let obj = wire.as_object().expect("document serialises to object");
        assert!(
            !obj.contains_key("features"),
            "K4.5.4e must not emit the features field: {wire}"
        );
        assert!(
            obj.contains_key("feature_calls"),
            "K4.5.4e must emit feature_calls: {wire}"
        );
    }

    #[test]
    fn document_roundtrips_through_json() {
        let mut doc = Document::default();
        let id = doc.allocate_id();
        doc.feature_calls.push(FeatureCall::new(
            "box",
            id,
            serde_json::json!({ "half_extents": [10.0, 10.0, 10.0] }),
        ));

        let json = serde_json::to_string(&doc).unwrap();
        let parsed: Document = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, doc.schema_version);
        assert_eq!(parsed.name, doc.name);
        assert_eq!(parsed.next_feature_id, doc.next_feature_id);
        assert_eq!(parsed.feature_calls, doc.feature_calls);
        assert!(parsed.features.is_empty());
    }

    #[test]
    fn transform_default_is_identity() {
        let t = Transform::default();
        assert!(t.is_identity());
        assert_eq!(t.translate, [0.0, 0.0, 0.0]);
        assert_eq!(t.rotation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(t.scale, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn allocate_id_is_monotonic() {
        let mut doc = Document::default();
        let a = doc.allocate_id();
        let b = doc.allocate_id();
        let c = doc.allocate_id();
        let an = a.as_u32_if_flat().unwrap();
        let bn = b.as_u32_if_flat().unwrap();
        let cn = c.as_u32_if_flat().unwrap();
        assert!(an < bn && bn < cn);
        assert_eq!(doc.next_feature_id, cn + 1);
    }

    #[test]
    fn feature_id_display_and_parse_round_trip() {
        let flat = FeatureId::from_u32(42);
        assert_eq!(flat.to_string(), "42");
        assert_eq!(FeatureId::from_str("42").unwrap(), flat);
        assert!(flat.is_flat_numeric());

        let child = flat.child("op");
        assert_eq!(child.to_string(), "42.op");
        assert_eq!(FeatureId::from_str("42.op").unwrap(), child);
        assert!(!child.is_flat_numeric());
        assert!(child.as_u32_if_flat().is_none());

        let grand = child.child("mask_0");
        assert_eq!(grand.to_string(), "42.op.mask_0");
        assert_eq!(FeatureId::from_str("42.op.mask_0").unwrap(), grand);
    }

    #[test]
    fn feature_id_serde_round_trip() {
        let flat = FeatureId::from_u32(7);
        let json = serde_json::to_string(&flat).unwrap();
        assert_eq!(json, "7");
        assert_eq!(serde_json::from_str::<FeatureId>("7").unwrap(), flat);

        let hier = FeatureId::from_u32(3).child("op");
        let json = serde_json::to_string(&hier).unwrap();
        assert_eq!(json, "\"3.op\"");
        assert_eq!(serde_json::from_str::<FeatureId>("\"3.op\"").unwrap(), hier);
    }

    // ─── FeatureCall (K4.5.4b) ──────────────────────────────────

    #[test]
    fn feature_call_builds_from_fields() {
        let call = FeatureCall::new(
            "box",
            FeatureId::from_u32(7),
            serde_json::json!({ "half_extents": [1.0, 2.0, 3.0] }),
        );
        assert_eq!(call.type_name, "box");
        assert_eq!(call.id, FeatureId::from_u32(7));
        assert_eq!(
            call.params,
            serde_json::json!({ "half_extents": [1.0, 2.0, 3.0] })
        );
    }

    #[test]
    fn feature_call_serde_shape_uses_type_field_not_type_name() {
        // §4.6 canonical wire shape: `{ "type": "...", "id": ...,
        // "params": {...} }`. The Rust field is called `type_name`
        // because `type` is a reserved keyword; the
        // `#[serde(rename)]` glue emits `type` on the wire.
        let call = FeatureCall::new(
            "sphere",
            FeatureId::from_u32(3),
            serde_json::json!({ "radius": 5.0 }),
        );
        let wire = serde_json::to_value(&call).unwrap();
        assert_eq!(wire.get("type"), Some(&serde_json::json!("sphere")));
        assert!(wire.get("type_name").is_none());
        assert_eq!(wire.get("id"), Some(&serde_json::json!(3)));
        assert_eq!(
            wire.get("params"),
            Some(&serde_json::json!({ "radius": 5.0 }))
        );
    }

    #[test]
    fn feature_call_round_trips_through_json() {
        let call = FeatureCall::new(
            "cylinder",
            FeatureId::from_u32(1).child("cap"),
            serde_json::json!({
                "radius": 4.0,
                "half_height": 10.0,
                "op": "difference"
            }),
        );
        let wire = serde_json::to_string(&call).unwrap();
        let parsed: FeatureCall = serde_json::from_str(&wire).unwrap();
        assert_eq!(parsed, call);
    }

    #[test]
    fn feature_call_params_default_to_null_when_missing() {
        // `#[serde(default)]` lets a call without a params field
        // still deserialise — handy for the K4.5 step-5 JS shell
        // that sometimes emits parameter-less calls during drag
        // previews.
        let wire = r#"{"type":"box","id":2}"#;
        let parsed: FeatureCall = serde_json::from_str(wire).unwrap();
        assert_eq!(parsed.type_name, "box");
        assert_eq!(parsed.id, FeatureId::from_u32(2));
        assert_eq!(parsed.params, serde_json::Value::Null);
    }

    #[test]
    fn feature_id_deserializer_accepts_both_shapes() {
        let from_int: FeatureId = serde_json::from_str("5").unwrap();
        assert_eq!(from_int, FeatureId::from_u32(5));
        let from_str: FeatureId = serde_json::from_str("\"5\"").unwrap();
        assert_eq!(from_str, FeatureId::from_u32(5));
    }

    #[test]
    fn primitive_kind_tags_match_constants() {
        assert_eq!(
            Primitive::Box { center: [0.0; 3], half_extents: [1.0; 3] }.kind_tag(),
            Primitive::KIND_BOX
        );
        assert_eq!(
            Primitive::Sphere { center: [0.0; 3], radius: 1.0 }.kind_tag(),
            Primitive::KIND_SPHERE
        );
        assert_eq!(
            Primitive::Cylinder { center: [0.0; 3], radius: 1.0, half_height: 1.0 }.kind_tag(),
            Primitive::KIND_CYLINDER
        );
    }

    #[test]
    fn bool_op_serializes_snake_case() {
        let json = serde_json::to_string(&BoolOp::Difference).unwrap();
        assert_eq!(json, "\"difference\"");
        let parsed: BoolOp = serde_json::from_str("\"intersect\"").unwrap();
        assert_eq!(parsed, BoolOp::Intersect);
    }

    #[test]
    fn bool_op_tags_match_constants() {
        assert_eq!(BoolOp::Union.tag(), BoolOp::TAG_UNION);
        assert_eq!(BoolOp::Difference.tag(), BoolOp::TAG_DIFFERENCE);
        assert_eq!(BoolOp::Intersect.tag(), BoolOp::TAG_INTERSECT);
    }

    // ─── K4.5.4f: Length / Angle / Query runtime types ────────

    #[test]
    fn length_wire_form_is_value_plus_unit() {
        let l = Length::mm(12.5);
        let wire = serde_json::to_value(l).unwrap();
        assert_eq!(wire, serde_json::json!({"value": 12.5, "unit": "mm"}));
        let parsed: Length =
            serde_json::from_value(serde_json::json!({"value": 5.0, "unit": "cm"}))
                .unwrap();
        assert_eq!(parsed, Length::cm(5.0));
    }

    #[test]
    fn length_to_mm_converts_every_unit() {
        assert_eq!(Length::mm(10.0).to_mm(), 10.0);
        assert_eq!(Length::cm(1.0).to_mm(), 10.0);
        assert_eq!(Length::m(2.5).to_mm(), 2500.0);
        assert!((Length::inches(1.0).to_mm() - 25.4).abs() < 1e-4);
    }

    #[test]
    fn length_rejects_unknown_unit() {
        let err = serde_json::from_value::<Length>(
            serde_json::json!({"value": 1.0, "unit": "furlong"}),
        );
        assert!(err.is_err());
    }

    #[test]
    fn angle_wire_form_is_value_plus_unit() {
        let a = Angle::deg(90.0);
        let wire = serde_json::to_value(a).unwrap();
        assert_eq!(wire, serde_json::json!({"value": 90.0, "unit": "deg"}));
        let parsed: Angle = serde_json::from_value(
            serde_json::json!({"value": 1.5, "unit": "rad"}),
        )
        .unwrap();
        assert_eq!(parsed, Angle::rad(1.5));
    }

    #[test]
    fn angle_to_rad_converts_both_units() {
        assert!((Angle::deg(180.0).to_rad() - std::f32::consts::PI).abs() < 1e-6);
        assert_eq!(Angle::rad(1.23).to_rad(), 1.23);
    }

    #[test]
    fn query_wire_form_is_bare_string() {
        let q = Query::new("qCreatedBy(box_1)");
        let wire = serde_json::to_value(&q).unwrap();
        assert_eq!(wire, serde_json::json!("qCreatedBy(box_1)"));
        let parsed: Query =
            serde_json::from_value(serde_json::json!("qFaceOf(extrude_3, top)"))
                .unwrap();
        assert_eq!(parsed.as_str(), "qFaceOf(extrude_3, top)");
    }
}
