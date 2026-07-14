//! Per-Body-B-Rep-Brücke: Weave/Import, Kanten/Flächen/Vertices als
//! JSON, Placement und Face-Index-Puffer.

use wasm_bindgen::prelude::*;

use crate::Kernel;
use crate::{quat_rotate};
#[wasm_bindgen]
impl Kernel {
    // ── Per-body *feature-based* B-Rep (sharp topology) ──────────

    /// Gate for the per-body B-Rep exports while weaving is suppressed
    /// (`setBrepSuppressed`): `true` when serving body `i` costs no weave —
    /// either suppression is off, or the body's cached B-Rep is still
    /// current (e.g. after a pure move, whose placement-invariant key is
    /// unchanged). `false` means "emit an empty payload".
    ///
    /// The bounds check is the gate keeping its own promise. A document can
    /// legitimately regenerate to ZERO bodies — an incomplete revolve warns
    /// and produces none (ticket ni56) — and JS may still ask for body 0.
    /// Without the check the `||` short-circuits past `bodies[i]` whenever
    /// suppression is off, the caller falls straight into `self.bodies[i]`,
    /// and the out-of-bounds panic ABORTS THE WHOLE WASM INSTANCE: the app
    /// dies with a bare `RuntimeError: unreachable` instead of the query
    /// simply coming back empty.
    fn brep_export_allowed(&mut self, i: usize) -> bool {
        i < self.bodies.len()
            && (!self.brep_suppressed || self.bodies[i].brep_is_current(&self.decoded))
    }

    /// Counts of the body's feature-based B-Rep entities as a JSON
    /// object: `{"vertices","edges","faces","shells","solids",
    /// "watertight"}`. `watertight` reflects the body's single outer
    /// shell's `is_closed` flag.
    ///
    /// This is the **real** B-Rep topology (sharp corners / edges /
    /// trimmed faces from the analytic weave). The B-Rep is woven
    /// lazily on first call and cached on the body, so this query is
    /// cheap on repeat (DL-30). Smooth-only bodies (e.g. a lone sphere)
    /// carry no sharp
    /// features and legitimately report zero/near-zero counts —
    /// feature-based B-Rep captures sharp topology only (DL-29).
    #[wasm_bindgen(js_name = bodyBrepCounts)]
    pub fn body_brep_counts(&mut self, i: usize) -> String {
        if !self.brep_export_allowed(i) {
            return serde_json::json!({
                "vertices": 0, "edges": 0, "faces": 0,
                "shells": 0, "solids": 0, "watertight": false,
            })
            .to_string();
        }
        // Split the borrow: `ensure_brep` needs `&mut bodies[i]` and
        // `&document.features` simultaneously, which a method on the
        // whole `Kernel` can't express without aliasing.
        let brep = self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let watertight = brep.shells.first().map(|s| s.is_closed).unwrap_or(false);
        serde_json::json!({
            "vertices": brep.vertex_count(),
            "edges": brep.edge_count(),
            "faces": brep.face_count(),
            "shells": brep.shell_count(),
            "solids": brep.solid_count(),
            "watertight": watertight,
        })
        .to_string()
    }

    /// Stable ids (see [`Kernel::body_id`]) of bodies whose woven B-Rep is
    /// NOT current — i.e. an export would either weave synchronously (no
    /// suppression) or serve an empty payload (suppressed). The async
    /// weaver pipeline polls this after every commit and farms the listed
    /// bodies out to the weaver worker. Refreshes placements as a side
    /// effect; never weaves.
    #[wasm_bindgen(js_name = bodyBrepStaleIds)]
    pub fn body_brep_stale_ids(&mut self) -> String {
        let mut stale: Vec<u32> = Vec::new();
        for i in 0..self.bodies.len() {
            if !self.bodies[i].brep_is_current(&self.decoded) {
                stale.push(self.body_id(i));
            }
        }
        serde_json::to_string(&stale).unwrap_or_else(|_| "[]".into())
    }

    /// WEAVER WORKER: weave body `body_id`'s B-Rep against the current
    /// document state and return it serialized as MessagePack
    /// `(placement-invariant key, BRep)` (named mode — the B-Rep's
    /// internally-tagged enums need a self-describing format), ready for
    /// [`Kernel::import_body_brep`] on the main thread. Returns an empty
    /// buffer when the id is unknown (the document changed under the
    /// request) or serialization fails — the importer treats both as a
    /// miss and the next scheduled weave supersedes.
    #[wasm_bindgen(js_name = weaveBodyBrep)]
    pub fn weave_body_brep(&mut self, body_id: u32) -> Vec<u8> {
        let Some(i) = (0..self.bodies.len()).find(|&i| self.body_id(i) == body_id)
        else {
            return Vec::new();
        };
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let Some((key, brep)) = self.bodies[i].brep.as_ref() else {
            return Vec::new();
        };
        let mut buf = Vec::new();
        let mut ser = rmp_serde::Serializer::new(&mut buf)
            .with_struct_map()
            .with_human_readable();
        use serde::Serialize as _;
        if (*key, brep).serialize(&mut ser).is_err() {
            return Vec::new();
        }
        buf
    }

    /// MAIN THREAD: install a worker-woven B-Rep (payload from
    /// [`Kernel::weave_body_brep`]) into the body's cache. The serialized
    /// placement-invariant key is validated against the CURRENT document
    /// state — a result that raced a newer edit simply misses, and the
    /// next scheduled weave supersedes it. On success the body version
    /// bumps and the mesh faces re-tag (a no-op in worker-meshing mode,
    /// where main-thread meshes are empty) so overlays and picking
    /// refetch. Returns whether anything was installed.
    #[wasm_bindgen(js_name = importBodyBrep)]
    pub fn import_body_brep(&mut self, body_id: u32, bytes: &[u8]) -> bool {
        use crate::brep::BRep;
        let Some(i) = (0..self.bodies.len()).find(|&i| self.body_id(i) == body_id)
        else {
            return false;
        };
        let mut de = rmp_serde::Deserializer::new(bytes).with_human_readable();
        use serde::Deserialize as _;
        let Ok((key, brep)) = <(u64, BRep)>::deserialize(&mut de) else {
            return false;
        };
        self.bodies[i].refresh_placement(&self.decoded);
        let current = crate::body::hash_body_local_payload(
            &self.decoded,
            &self.bodies[i].feature_indices,
            &self.bodies[i].placement,
        );
        if current != key {
            return false; // raced a newer edit — superseded
        }
        if matches!(&self.bodies[i].brep, Some((k, _)) if *k == key) {
            return false; // already current — avoid version churn
        }
        self.bodies[i].brep = Some((key, brep));
        self.bodies[i].version = self.bodies[i].version.wrapping_add(1);
        self.retag_mesh_faces();
        self.mesh_version = self.mesh_version.wrapping_add(1);
        true
    }

    /// Per-body analytic B-Rep edges, sampled to **world-space** polylines for
    /// viewport rendering and hover. Returns a JSON array; each edge is
    /// `{"a":<branch u32>,"b":<branch u32>,"idHash":<decimal string>,
    /// "points":[x,y,z, …],"controlPoints":[x,y,z, …],"degree":<usize>}`,
    /// where `points` is the dense tessellation (for the wireframe) and
    /// `controlPoints`/`degree` are the sparse structural points for the debug
    /// points overlay (on-curve nodes sit at every `degree`-th control point).
    /// `a`/`b` are the canonical
    /// colliding-branch signature. Those two ids are
    /// the same `as_u32()` branch values that [`Kernel::pick`] reports in its
    /// `contributors` array, so the frontend can match a hovered edge to its
    /// polyline. `idHash` is the regeneration-stable
    /// [`crate::brep::naming::TopoRef`] edge id (decimal string, like the
    /// faces/vertices exports), so a click on an edge can emit
    /// `qEdge(<idHash>)`. Points are sampled in the body-local frame and folded to world
    /// by the body's `placement` (DL-56), exactly like the STEP export. Reuses
    /// the same lazily-woven, cached B-Rep as the counts / STEP queries.
    #[wasm_bindgen(js_name = bodyBrepEdges)]
    pub fn body_brep_edges(&mut self, i: usize) -> String {
        use crate::brep::naming::TopoRef;
        use crate::brep::EdgeId;

        if !self.brep_export_allowed(i) {
            return "[]".into();
        }
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let placement = self.bodies[i].placement;
        let brep = self.bodies[i].brep_cached().expect("woven above");
        let s = placement.scale[0];
        let mut edges = Vec::with_capacity(brep.edges.len());
        // Fold a body-local point chain to world: world = R·(s·p) + t
        // (rigid + uniform-scale placement), flattened to `[x,y,z, …]`.
        let to_world = |pts: &[[f64; 3]]| -> Vec<f32> {
            let mut out = Vec::with_capacity(pts.len() * 3);
            for p in pts {
                let scaled = [p[0] as f32 * s, p[1] as f32 * s, p[2] as f32 * s];
                let r = quat_rotate(placement.rotation, scaled);
                out.push(r[0] + placement.translate[0]);
                out.push(r[1] + placement.translate[1]);
                out.push(r[2] + placement.translate[2]);
            }
            out
        };
        // Edge → adjacent face ARENA indices (same ordering as
        // `bodyBrepFaces`). Selection display needs the face-precise
        // association: coplanar split faces share one branch signature
        // (the coincidence canonicalisation), so matching edges by
        // `a`/`b` alone lights up the whole coplanar family (ticket b91c).
        let mut edge_faces: Vec<Vec<u32>> = vec![Vec::new(); brep.edges.len()];
        for (fk, face) in brep.faces.iter().enumerate() {
            for lp in std::iter::once(&face.outer).chain(face.inner.iter()) {
                for (eid, _) in &lp.edges {
                    let v = &mut edge_faces[eid.0 as usize];
                    if !v.contains(&(fk as u32)) {
                        v.push(fk as u32);
                    }
                }
            }
        }
        for (k, edge) in brep.edges.iter().enumerate() {
            let topo = TopoRef::from_edge(brep, EdgeId(k as u32));
            let id_hash = topo.id_hash;
            let points = to_world(&crate::brep::edge_sampling::sample_edge(&edge.geometry));
            // Structural points for the debug overlay: the few control points
            // that define the curve, plus its degree (on-curve nodes sit at
            // every `degree`-th control point). Far sparser than `points`.
            let (cps_local, degree) =
                crate::brep::edge_sampling::edge_control_structure(&edge.geometry);
            let control_points = to_world(&cps_local);
            edges.push(serde_json::json!({
                "a": edge.signature.0.as_u32(),
                "b": edge.signature.1.as_u32(),
                "idHash": id_hash.to_string(),
                "query": crate::queries::element_query(&topo).to_string(),
                "points": points,
                "controlPoints": control_points,
                "degree": degree,
                "faces": edge_faces[k],
            }));
        }
        serde_json::Value::Array(edges).to_string()
    }

    /// **Debug only.** Per-body topology-octree leaf cells for the viewport's
    /// octree-cell overlay. Returns a JSON object
    /// `{"translate":[x,y,z],"rotation":[x,y,z,w],"scale":s,
    /// "cells":[{"min":[x,y,z],"max":[x,y,z],"class":<0|1|2>}, …]}`, where the
    /// cells are in the body-LOCAL frame and `translate`/`rotation`/`scale`
    /// are the body's rigid+uniform-scale placement (apply them as a single
    /// world transform on the frontend — a group matrix — to fold the cells to
    /// world, exactly like the analytic faces are local + placement).
    /// `class` is `0`=Face, `1`=Edge, `2`=Vertex. The octree is rebuilt on
    /// every call (not cached); this is a diagnostic path, not the hot path.
    #[wasm_bindgen(js_name = bodyOctreeCells)]
    pub fn body_octree_cells(&mut self, i: usize) -> String {
        if !self.brep_export_allowed(i) {
            // Suppressed-but-present body: hand back its placement with no
            // cells, so a pure move still positions the (empty) overlay. No
            // body at all: there is no placement to report — `i` is simply
            // out of range, so report nothing rather than indexing into it.
            let p = self.bodies.get(i).map(|b| b.placement).unwrap_or_default();
            return serde_json::json!({
                "translate": p.translate,
                "rotation": p.rotation,
                "scale": p.scale[0],
                "cells": [],
            })
            .to_string();
        }
        // Refresh placement off the (cached) weave, then rebuild the octree.
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let placement = self.bodies[i].placement;
        let cells = self.bodies[i].debug_octree_cells(&self.decoded);
        let cells_json: Vec<serde_json::Value> = cells
            .iter()
            .map(|(min, max, class)| {
                serde_json::json!({ "min": min, "max": max, "class": class })
            })
            .collect();
        serde_json::json!({
            "translate": placement.translate,
            "rotation": placement.rotation,
            "scale": placement.scale[0],
            "cells": cells_json,
        })
        .to_string()
    }

    /// Per-body feature-based B-Rep **faces**, in the body-LOCAL frame, as a
    /// JSON array — one entry per `brep.faces[k]` (Brocken A1). Each entry:
    /// `{"faceIndex","idHash","kind":"face","branchId","featureId",
    /// "surfaceKind","centroidLocal":[x,y,z],"normalLocal":[x,y,z]}`.
    ///
    /// `idHash` is the regeneration-stable [`crate::brep::naming::TopoRef`]
    /// id (FNV-1a over the sorted branch signature + spatial index),
    /// serialised as a **decimal string** because a `u64` is unsafe as a JS
    /// `Number`. `faceIndex` indexes the same ordering as the per-vertex
    /// `face_index` mesh tags (`body_face_index_ptr`), so a hovered triangle's
    /// tag selects this entry. Geometry stays body-local; the caller folds the
    /// body `placement` (`bodyPlacement`) in for world coordinates.
    #[wasm_bindgen(js_name = bodyBrepFaces)]
    pub fn body_brep_faces(&mut self, i: usize) -> String {
        use crate::brep::face_export::{face_centroid_normal_local, surface_kind, surface_params};
        use crate::brep::naming::TopoRef;
        use crate::brep::FaceId;

        if !self.brep_export_allowed(i) {
            return "[]".into();
        }
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let brep = self.bodies[i].brep_cached().expect("woven above");
        let mut faces = Vec::with_capacity(brep.faces.len());
        for (k, face) in brep.faces.iter().enumerate() {
            let topo = TopoRef::from_face(brep, FaceId(k as u32));
            let id_hash = topo.id_hash;
            let (centroid, normal) = face_centroid_normal_local(brep, FaceId(k as u32));
            faces.push(serde_json::json!({
                "faceIndex": k,
                "idHash": id_hash.to_string(),
                "query": crate::queries::element_query(&topo).to_string(),
                "kind": "face",
                "branchId": face.branch_id.as_u32(),
                "featureId": face.branch_id.feature_id(),
                "surfaceKind": surface_kind(&face.surface),
                "surfaceParams": surface_params(&face.surface),
                "centroidLocal": centroid,
                "normalLocal": normal,
            }));
        }
        serde_json::Value::Array(faces).to_string()
    }

    /// Per-body feature-based B-Rep **vertices**, in the body-LOCAL frame, as a
    /// JSON array — one entry per `brep.vertices[k]` (Brocken A1). Each entry:
    /// `{"vertexIndex","idHash","kind":"vertex","positionLocal":[x,y,z],
    /// "branches":[<u32 packed>, …]}`. `idHash` is the stable
    /// [`crate::brep::naming::TopoRef`] vertex id as a decimal string.
    #[wasm_bindgen(js_name = bodyBrepVertices)]
    pub fn body_brep_vertices(&mut self, i: usize) -> String {
        use crate::brep::naming::TopoRef;
        use crate::brep::VertexId;

        if !self.brep_export_allowed(i) {
            return "[]".into();
        }
        self.bodies[i].ensure_brep(&self.decoded, self.parametric_weave);
        let brep = self.bodies[i].brep_cached().expect("woven above");
        let mut verts = Vec::with_capacity(brep.vertices.len());
        for (k, vertex) in brep.vertices.iter().enumerate() {
            let topo = TopoRef::from_vertex(brep, VertexId(k as u32));
            let id_hash = topo.id_hash;
            let branches: Vec<u32> = vertex.branches.iter().map(|b| b.as_u32()).collect();
            verts.push(serde_json::json!({
                "vertexIndex": k,
                "idHash": id_hash.to_string(),
                "query": crate::queries::element_query(&topo).to_string(),
                "kind": "vertex",
                "positionLocal": vertex.position,
                "branches": branches,
            }));
        }
        serde_json::Value::Array(verts).to_string()
    }

    /// The body's rigid + uniform-scale **placement** as JSON
    /// `{"translate":[x,y,z],"rotation":[x,y,z,w],"scale":[sx,sy,sz]}`
    /// (Brocken A2). B-Reps stay body-LOCAL and an affine transform never
    /// invalidates them; the frontend composes this placement back in
    /// (`localToWorld`) to obtain world coordinates. Placement is derived
    /// straight from the features — it never depends on the weave, so this
    /// refreshes it directly without ever triggering a B-Rep build.
    #[wasm_bindgen(js_name = bodyPlacement)]
    pub fn body_placement(&mut self, i: usize) -> String {
        self.bodies[i].refresh_placement(&self.decoded);
        let p = self.bodies[i].placement;
        serde_json::json!({
            "translate": p.translate,
            "rotation": p.rotation,
            "scale": p.scale,
        })
        .to_string()
    }

    /// Pointer to the per-**triangle** B-Rep **face index** buffer (Brocken
    /// A3): one `u32` per mesh triangle giving the arena index of the B-Rep
    /// face that triangle lies on (same ordering as `bodyBrepFaces`), or
    /// `u32::MAX` when unresolved. Length is `bodyTriangleCount` (==
    /// `bodyIndexCount / 3`). A hovered/clicked triangle resolves to its stable
    /// face by indexing this buffer with the triangle id. Per-triangle (not
    /// per-vertex) because planar simplification leaves flat faces with only
    /// shared boundary vertices, so vertices can't be uniquely owned by a face
    /// — triangle centroids always can. Populated during the mesh rebuild; an
    /// un-tagged body (e.g. a passthrough imported mesh) exposes an empty
    /// buffer (null pointer, length 0).
    #[wasm_bindgen(js_name = bodyFaceIndexPtr)]
    pub fn body_face_index_ptr(&self, i: usize) -> *const u32 {
        self.bodies[i].mesh.face_index.as_ptr()
    }

    /// Length of the per-triangle face-index buffer (`bodyFaceIndexPtr`).
    /// Equals `bodyTriangleCount` for a tagged mesh, or 0 when the body carries
    /// no face tags.
    #[wasm_bindgen(js_name = bodyFaceIndexLen)]
    pub fn body_face_index_len(&self, i: usize) -> usize {
        self.bodies[i].mesh.face_index.len()
    }


    /// Suppress every B-Rep weave while a gizmo drag streams throttled
    /// parameter commits (web setting "B-Rep rebuild while dragging",
    /// default suppressed). While on, regen skips the eager face-tagging
    /// weave and the B-Rep exports (`bodyBrepEdges`/`Faces`/`Vertices`/
    /// `Counts`) serve only a still-current cache — a body whose geometry
    /// changed under the drag reports empty payloads until the drag ends.
    /// Turning the flag off bumps the version of every body whose B-Rep
    /// went stale (the frontend's refetch signal) and re-runs the
    /// face-tagging pass, weaving each stale body exactly once. Returns
    /// `false` if the flag was already at `on`.
    #[wasm_bindgen(js_name = setBrepSuppressed)]
    pub fn set_brep_suppressed(&mut self, on: bool) -> bool {
        if self.brep_suppressed == on {
            return false;
        }
        self.brep_suppressed = on;
        if !on {
            let mut any_stale = false;
            for i in 0..self.bodies.len() {
                if !self.bodies[i].brep_is_current(&self.decoded) {
                    self.bodies[i].version = self.bodies[i].version.wrapping_add(1);
                    any_stale = true;
                }
            }
            self.retag_mesh_faces();
            if any_stale {
                self.mesh_version = self.mesh_version.wrapping_add(1);
            }
        }
        true
    }

}
