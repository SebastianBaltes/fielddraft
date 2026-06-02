//! STL export — ASCII + Binary.
//!
//! K7.2 — first real output path for the kernel. Walks every
//! body's Dual-Contouring mesh and emits either an ASCII `solid`
//! block or the binary 80-byte-header + u32-triangle-count + 50-
//! byte-per-triangle record format used by every mainstream
//! slicer.
//!
//! The kernel owns the mesh buffers as flat `Vec<f32>` /
//! `Vec<u32>` per body — [`Mesh::positions`], [`Mesh::normals`],
//! [`Mesh::indices`] — so the encoder just has to iterate
//! triangles, pull three vertices each time and write them out.
//! Normals are recomputed from the triangle cross product at
//! export time rather than reading the per-vertex normals,
//! because STL's per-triangle normal field is what every slicer
//! expects, and the DC vertex normals are smoothed rather than
//! per-facet.
//!
//! Units: the mesh lives in millimetres (same as every other
//! length in FieldDraft), and STL itself is unit-less. The
//! slicer takes the model's length unit from the user's import
//! settings; millimetres is the universal default so the
//! exported file lines up out of the box.
//!
//! Deterministic output: bodies are iterated in the order the
//! caller passes them in. Within a body, triangles come out in
//! the `indices` order they appear in the mesh. No sorting, no
//! orientation fixing — the DC mesher already produces
//! outward-facing, right-handed triangles.

use crate::dc::Mesh;

/// Write every triangle in every mesh as ASCII STL. The
/// `solid` name is hard-coded to `fielddraft`; downstream
/// tooling that cares can retag via find-and-replace.
pub fn export_ascii(meshes: &[&Mesh]) -> String {
    let mut out = String::new();
    out.push_str("solid fielddraft\n");
    for mesh in meshes {
        append_mesh_ascii(&mut out, mesh);
    }
    out.push_str("endsolid fielddraft\n");
    out
}

fn append_mesh_ascii(out: &mut String, mesh: &Mesh) {
    use std::fmt::Write;
    for tri in mesh.indices.chunks_exact(3) {
        let Some((a, b, c)) = load_triangle(mesh, tri) else {
            continue;
        };
        let n = face_normal(a, b, c);
        writeln!(
            out,
            "  facet normal {:.6e} {:.6e} {:.6e}",
            n[0], n[1], n[2]
        )
        .unwrap();
        out.push_str("    outer loop\n");
        writeln!(
            out,
            "      vertex {:.6e} {:.6e} {:.6e}",
            a[0], a[1], a[2]
        )
        .unwrap();
        writeln!(
            out,
            "      vertex {:.6e} {:.6e} {:.6e}",
            b[0], b[1], b[2]
        )
        .unwrap();
        writeln!(
            out,
            "      vertex {:.6e} {:.6e} {:.6e}",
            c[0], c[1], c[2]
        )
        .unwrap();
        out.push_str("    endloop\n");
        out.push_str("  endfacet\n");
    }
}

/// Write every triangle in every mesh as binary STL. Layout is
/// the canonical 3D printing format:
///
/// ```text
/// header   — 80 bytes  — arbitrary, filled with a FieldDraft tag
/// triangle count — u32 little-endian
/// per triangle:
///   normal  — 3 × f32 little-endian
///   vertex  — 3 × 3 × f32 little-endian
///   attribute byte count — u16 little-endian (always 0)
/// ```
pub fn export_binary(meshes: &[&Mesh]) -> Vec<u8> {
    // Collect valid triangles first so the header count matches
    // the actual number of records (load_triangle skips
    // out-of-bounds indices).
    let triangles: Vec<([f32; 3], [f32; 3], [f32; 3])> = meshes
        .iter()
        .flat_map(|mesh| {
            mesh.indices
                .chunks_exact(3)
                .filter_map(|tri| load_triangle(mesh, tri))
        })
        .collect();
    let triangle_count = triangles.len() as u32;

    let mut out = Vec::with_capacity(
        80 + 4 + triangle_count as usize * 50,
    );
    // 80-byte header. Any non-"solid" prefix tells parsers
    // "this is binary, not ASCII". We pad with zeros after the
    // tag; slicers typically ignore the payload but some log it.
    let tag = b"FieldDraft STL export (binary)";
    let mut header = [0u8; 80];
    let n = tag.len().min(80);
    header[..n].copy_from_slice(&tag[..n]);
    out.extend_from_slice(&header);
    out.extend_from_slice(&triangle_count.to_le_bytes());

    for (a, b, c) in &triangles {
        let normal = face_normal(*a, *b, *c);
        for v in normal {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in *a {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in *b {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in *c {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }

    out
}

// ─── helpers ────────────────────────────────────────────────────

fn load_triangle(
    mesh: &Mesh,
    tri: &[u32],
) -> Option<([f32; 3], [f32; 3], [f32; 3])> {
    let [i0, i1, i2] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
    let v = |i: usize| -> Option<[f32; 3]> {
        let base = i.checked_mul(3)?;
        let x = *mesh.positions.get(base)?;
        let y = *mesh.positions.get(base + 1)?;
        let z = *mesh.positions.get(base + 2)?;
        Some([x, y, z])
    };
    Some((v(i0)?, v(i1)?, v(i2)?))
}

fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let nx = u[1] * v[2] - u[2] * v[1];
    let ny = u[2] * v[0] - u[0] * v[2];
    let nz = u[0] * v[1] - u[1] * v[0];
    let len = (nx * nx + ny * ny + nz * nz).sqrt();
    if len > 0.0 {
        [nx / len, ny / len, nz / len]
    } else {
        [0.0, 0.0, 0.0]
    }
}

// ─── tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_mesh() -> Mesh {
        // One up-facing triangle in the XZ plane.
        Mesh {
            positions: vec![
                0.0, 0.0, 0.0,
                1.0, 0.0, 0.0,
                0.0, 0.0, 1.0,
            ],
            normals: vec![0.0; 9],
            indices: vec![0, 1, 2],
            classifications: vec![0; 3],
            contributors: vec![],
            face_index: vec![],
        }
    }

    fn two_triangle_mesh() -> Mesh {
        // Square split into two triangles, facing +Y.
        Mesh {
            positions: vec![
                0.0, 0.0, 0.0,
                1.0, 0.0, 0.0,
                1.0, 0.0, 1.0,
                0.0, 0.0, 1.0,
            ],
            normals: vec![0.0; 12],
            indices: vec![0, 1, 2, 0, 2, 3],
            classifications: vec![0; 4],
            contributors: vec![],
            face_index: vec![],
        }
    }

    #[test]
    fn ascii_empty_mesh_still_wraps_in_solid() {
        let mesh = Mesh {
            positions: vec![],
            normals: vec![],
            indices: vec![],
            classifications: vec![],
            contributors: vec![],
            face_index: vec![],
        };
        let text = export_ascii(&[&mesh]);
        assert!(text.starts_with("solid fielddraft"));
        assert!(text.trim_end().ends_with("endsolid fielddraft"));
        assert!(!text.contains("facet"));
    }

    #[test]
    fn ascii_single_triangle_has_one_facet_block() {
        let mesh = triangle_mesh();
        let text = export_ascii(&[&mesh]);
        assert_eq!(text.matches("facet normal").count(), 1);
        assert_eq!(text.matches("outer loop").count(), 1);
        assert_eq!(text.matches("vertex").count(), 3);
        // Facet normal for this XZ-plane triangle with winding
        // (0,0,0)→(1,0,0)→(0,0,1): u×v = (0,-1,0).
        assert!(
            text.contains("facet normal 0.000000e0 -1.000000e0 0.000000e0"),
            "normal string missing from:\n{text}",
        );
    }

    #[test]
    fn ascii_multi_mesh_emits_every_triangle() {
        let a = triangle_mesh();
        let b = two_triangle_mesh();
        let text = export_ascii(&[&a, &b]);
        // 1 from first mesh + 2 from second.
        assert_eq!(text.matches("facet normal").count(), 3);
    }

    #[test]
    fn binary_header_layout_is_80_plus_4_plus_per_triangle() {
        let mesh = two_triangle_mesh();
        let bytes = export_binary(&[&mesh]);
        // 80-byte header + 4-byte count + 50 bytes per triangle.
        assert_eq!(bytes.len(), 80 + 4 + 2 * 50);
        // Triangle count at bytes 80..84, little-endian.
        let count = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]);
        assert_eq!(count, 2);
        // Header tag: `FieldDraft STL export (binary)`
        assert!(bytes[..30].starts_with(b"FieldDraft STL"));
    }

    #[test]
    fn binary_triangle_normal_and_vertices_round_trip() {
        let mesh = triangle_mesh();
        let bytes = export_binary(&[&mesh]);
        // Skip header + count, parse the single triangle record.
        let mut cursor = 84;
        let read_f32 = |b: &[u8], o: usize| -> f32 {
            f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
        };
        let nx = read_f32(&bytes, cursor);
        let ny = read_f32(&bytes, cursor + 4);
        let nz = read_f32(&bytes, cursor + 8);
        cursor += 12;
        let ax = read_f32(&bytes, cursor);
        let az = read_f32(&bytes, cursor + 8);
        cursor += 12;
        let bx = read_f32(&bytes, cursor);
        cursor += 12;
        let cz = read_f32(&bytes, cursor + 8);
        // Normal: (0, -1, 0). Vertex positions: a=(0,0,0),
        // b=(1,0,0), c=(0,0,1).
        assert!((nx - 0.0).abs() < 1e-6);
        assert!((ny + 1.0).abs() < 1e-6);
        assert!((nz - 0.0).abs() < 1e-6);
        assert!((ax - 0.0).abs() < 1e-6);
        assert!((az - 0.0).abs() < 1e-6);
        assert!((bx - 1.0).abs() < 1e-6);
        assert!((cz - 1.0).abs() < 1e-6);
    }

    #[test]
    fn binary_attribute_byte_count_is_zero_per_triangle() {
        let mesh = triangle_mesh();
        let bytes = export_binary(&[&mesh]);
        // The attribute count sits at the end of each 50-byte
        // triangle record. Triangle 0 starts at offset 84 and
        // runs 50 bytes; the u16 attr field is at 84 + 48 = 132.
        let attr = u16::from_le_bytes([bytes[132], bytes[133]]);
        assert_eq!(attr, 0);
    }

    #[test]
    fn face_normal_returns_zeros_for_degenerate_triangles() {
        let n = super::face_normal([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]);
        assert_eq!(n, [0.0, 0.0, 0.0]);
    }
}
