//! Serverseitiges CGAL-Meshing, Vorstufe (docs/CGAL-SERVER-MESHING-PLAN.md,
//! P2): aus einem Part-`document.json` das Interchange-File für
//! `fd-cgal-mesh` erzeugen — pro Body das SDF-Opcode-Programm (bit-exakt,
//! f32-Hex), die Welt-AABB und die B-Rep-Kanten als world-space Polylines
//! (die geschützten 1D-Features für CGAL `Mesh_3`).
//!
//! Läuft NATIV (kein wasm): der Mesher-Service ruft das dünne Binary
//! `fd-prepare` (src/bin/fd_prepare.rs), das hierher delegiert. Die
//! wasm-bindgen-Wrapper (`load_json` etc.) werden bewusst umgangen —
//! `JsError::new` panict off-wasm; stattdessen dieselben Internals wie die
//! nativen Kernel-Tests (`document` setzen → `bootstrap_features_from_calls`
//! → `rebuild_packed`).
//!
//! Format (Text, eine Anweisung pro Zeile; alle f32 als 8-stellige
//! Hex-Bits — dieselbe bit-exakte Konvention wie `gen_golden`/`check_interp`):
//!
//! ```text
//! FDMESH 1
//! PARAMS <params-json, eine Zeile, opak durchgereicht>
//! NBODIES <n>
//! BODY <i>
//! AABB <6× hex-f32>
//! NOPS <n>
//! OP <hex-f32>
//! …
//! NEDGES <k>
//! EDGE <npts> <hex-f32 x y z … npts-mal>
//! …
//! END
//! ```

use std::fmt::Write as _;

/// Baut das komplette Interchange-File für ein Part-Dokument. `params_json`
/// wird unverändert in die PARAMS-Zeile übernommen (Zeilenumbrüche
/// entfernt) — die Interpretation gehört der C++-Seite.
///
/// Fehler sind Strings (fürs CLI-stderr): ungültiges Dokument,
/// Regenerations-Fehler oder ein Body, den Phase 1 nicht unterstützt
/// (gebackene Volumes / OP_PRIM_TEX3D).
pub fn prepare_interchange(document_json: &str, params_json: &str) -> Result<String, String> {
    let mut k = crate::Kernel::new();
    k.document = serde_json::from_str(document_json)
        .map_err(|e| format!("invalid document json: {e}"))?;
    k.bootstrap_features_from_calls()
        .map_err(|e| format!("feature regeneration failed: {e:?}"))?;
    k.rebuild_packed();

    let mut out = String::new();
    out.push_str("FDMESH 1\n");
    let params_flat: String = params_json
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let _ = writeln!(out, "PARAMS {}", params_flat.trim());
    let _ = writeln!(out, "NBODIES {}", k.bodies.len());

    let hex = |v: f32| format!("{:08x}", v.to_bits());

    for bi in 0..k.bodies.len() {
        let (bounded, _) = crate::body::build_bounded_with_primitives(
            &k.decoded,
            &k.bodies[bi].feature_indices,
        );
        let compiled = crate::sdf_compiler::compile_body(&bounded);
        if !compiled.volumes.is_empty() {
            return Err(format!(
                "body {bi} uses baked volumes (OP_PRIM_TEX3D) — not supported by \
                 server meshing yet"
            ));
        }

        let _ = writeln!(out, "BODY {bi}");
        let a = &compiled.aabb;
        let _ = writeln!(
            out,
            "AABB {} {} {} {} {} {}",
            hex(a.min[0]),
            hex(a.min[1]),
            hex(a.min[2]),
            hex(a.max[0]),
            hex(a.max[1]),
            hex(a.max[2])
        );
        let _ = writeln!(out, "NOPS {}", compiled.opcodes.len());
        for op in &compiled.opcodes {
            let _ = writeln!(out, "OP {:08x}", op.to_bits());
        }

        // B-Rep-Kanten als world-space Polylines (Placement gefaltet) —
        // identische Faltung wie `bodyBrepEdges`/der Mesher-Skelett-Pfad.
        k.bodies[bi].ensure_brep(&k.decoded, true);
        let placement = k.bodies[bi].placement;
        let s = placement.scale[0];
        let to_world = |p: [f64; 3]| -> [f32; 3] {
            let scaled = [p[0] as f32 * s, p[1] as f32 * s, p[2] as f32 * s];
            let r = crate::quat_rotate(placement.rotation, scaled);
            [
                r[0] + placement.translate[0],
                r[1] + placement.translate[1],
                r[2] + placement.translate[2],
            ]
        };
        let edges: Vec<Vec<[f32; 3]>> = match k.bodies[bi].brep_cached() {
            Some(brep) => brep
                .edges
                .iter()
                .map(|e| {
                    crate::brep::edge_sampling::sample_edge(&e.geometry)
                        .into_iter()
                        .map(to_world)
                        .collect()
                })
                .filter(|pts: &Vec<[f32; 3]>| pts.len() >= 2)
                .collect(),
            None => Vec::new(),
        };
        let _ = writeln!(out, "NEDGES {}", edges.len());
        for pts in &edges {
            let mut line = format!("EDGE {}", pts.len());
            for p in pts {
                let _ = write!(line, " {} {} {}", hex(p[0]), hex(p[1]), hex(p[2]));
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.push_str("END\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cylinder document must yield one body with a non-empty opcode
    /// program, a finite AABB and its two rim circles as EDGE polylines.
    #[test]
    fn cylinder_document_produces_opcodes_and_edges() {
        let doc = serde_json::json!({
            "schema_version": 2,
            "name": "cyl",
            "next_feature_id": 2,
            "feature_calls": [
                { "type": "cylinder", "id": "1", "params": {
                    "radius": 10.0, "half_height": 10.0,
                    "translate": [0.0, 0.0, 0.0], "op": "new" } }
            ]
        })
        .to_string();
        let out = prepare_interchange(&doc, r#"{"cellSizeMm":1.0}"#).unwrap();
        assert!(out.starts_with("FDMESH 1\n"));
        assert!(out.contains("NBODIES 1"));
        let nops: usize = out
            .lines()
            .find(|l| l.starts_with("NOPS "))
            .and_then(|l| l[5..].parse().ok())
            .unwrap();
        assert!(nops > 0, "opcode program must be non-empty");
        let nedges: usize = out
            .lines()
            .find(|l| l.starts_with("NEDGES "))
            .and_then(|l| l[7..].parse().ok())
            .unwrap();
        assert_eq!(nedges, 2, "cylinder has two rim circles");
        // Every EDGE line parses back to finite f32 triples.
        for l in out.lines().filter(|l| l.starts_with("EDGE ")) {
            let mut it = l.split_whitespace().skip(1);
            let n: usize = it.next().unwrap().parse().unwrap();
            let coords: Vec<f32> = it
                .map(|h| f32::from_bits(u32::from_str_radix(h, 16).unwrap()))
                .collect();
            assert_eq!(coords.len(), n * 3);
            assert!(coords.iter().all(|c| c.is_finite()));
        }
    }

    /// Baked-volume bodies (imported meshes) are rejected with a clear
    /// message instead of shipping an opcode stream C++ cannot evaluate.
    #[test]
    fn invalid_document_is_a_clean_error() {
        assert!(prepare_interchange("{not json", "{}").is_err());
    }
}
