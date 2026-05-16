//! 3MF export — Multi-body ZIP archive with per-body colors.
//!
//! M14a — produces a valid 3MF (3D Manufacturing Format) file that
//! every mainstream slicer reads: Bambu Studio, PrusaSlicer, Cura,
//! OrcaSlicer.
//!
//! The archive contains three entries (STORE, no DEFLATE — keeps
//! the implementation dependency-free while the mesh XML is
//! typically the size bottleneck, not the boilerplate):
//!
//!   [Content_Types].xml   — MIME declarations
//!   _rels/.rels           — root relationship → 3D model
//!   3D/3dmodel.model      — mesh data + materials
//!
//! Each body becomes a separate `<object>` with its own material
//! from a shared `<basematerials>` table. The per-body color
//! palette mirrors `web/src/meshMaterial.ts` BODY_COLORS.

use crate::dc::Mesh;
use std::fmt::Write;

const BODY_COLORS: &[[f32; 3]] = &[
    [0.55, 0.65, 0.85], // blue (default)
    [0.65, 0.85, 0.55], // green
    [0.85, 0.55, 0.65], // rose
    [0.85, 0.75, 0.50], // amber
    [0.55, 0.80, 0.80], // teal
    [0.75, 0.55, 0.85], // violet
    [0.85, 0.85, 0.55], // lime
    [0.55, 0.70, 0.70], // slate
];

fn color_hex(idx: usize) -> String {
    let [r, g, b] = BODY_COLORS[idx % BODY_COLORS.len()];
    let to_u8 = |v: f32| (v * 255.0).round().min(255.0) as u8;
    format!("#{:02X}{:02X}{:02X}", to_u8(r), to_u8(g), to_u8(b))
}

pub fn export_3mf(meshes: &[&Mesh]) -> Vec<u8> {
    let content_types = content_types_xml();
    let rels = rels_xml();
    let model = model_xml(meshes);

    let mut zip = ZipWriter::new();
    zip.add("[Content_Types].xml", content_types.as_bytes());
    zip.add("_rels/.rels", rels.as_bytes());
    zip.add("3D/3dmodel.model", model.as_bytes());
    zip.finish()
}

// ─── XML builders ──────────────────────────────────────────────

fn content_types_xml() -> String {
    concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n",
        "  <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\" />\n",
        "  <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\" />\n",
        "</Types>\n",
    ).to_string()
}

fn rels_xml() -> String {
    concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n",
        "  <Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\" />\n",
        "</Relationships>\n",
    ).to_string()
}

fn model_xml(meshes: &[&Mesh]) -> String {
    let mut xml = String::with_capacity(meshes.len() * 4096);

    xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    xml.push_str("<model unit=\"millimeter\" xml:lang=\"en-US\"\n");
    xml.push_str("       xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\"\n");
    xml.push_str("       xmlns:m=\"http://schemas.microsoft.com/3dmanufacturing/material/2015/02\">\n");
    xml.push_str("  <metadata name=\"Application\">FieldDraft</metadata>\n");
    xml.push_str("  <resources>\n");

    // basematerials — one entry per non-empty body
    let non_empty: Vec<(usize, &&Mesh)> = meshes
        .iter()
        .enumerate()
        .filter(|(_, m)| !m.indices.is_empty())
        .collect();

    if !non_empty.is_empty() {
        xml.push_str("    <basematerials id=\"1\">\n");
        for (body_idx, _) in &non_empty {
            let _ = writeln!(
                xml,
                "      <base name=\"Body {}\" displaycolor=\"{}\" />",
                body_idx,
                color_hex(*body_idx),
            );
        }
        xml.push_str("    </basematerials>\n");
    }

    // one <object> per non-empty body
    let mut build_items = Vec::new();
    for (seq, (_body_idx, mesh)) in non_empty.iter().enumerate() {
        let obj_id = seq + 2; // id=1 is basematerials
        build_items.push(obj_id);

        let _ = writeln!(
            xml,
            "    <object id=\"{}\" type=\"model\" pid=\"1\" pindex=\"{}\">",
            obj_id, seq,
        );
        xml.push_str("      <mesh>\n");

        // vertices
        xml.push_str("        <vertices>\n");
        let vert_count = mesh.positions.len() / 3;
        for v in 0..vert_count {
            let base = v * 3;
            let _ = writeln!(
                xml,
                "          <vertex x=\"{:.6}\" y=\"{:.6}\" z=\"{:.6}\" />",
                mesh.positions[base],
                mesh.positions[base + 1],
                mesh.positions[base + 2],
            );
        }
        xml.push_str("        </vertices>\n");

        // triangles
        xml.push_str("        <triangles>\n");
        for tri in mesh.indices.chunks_exact(3) {
            let _ = writeln!(
                xml,
                "          <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" />",
                tri[0], tri[1], tri[2],
            );
        }
        xml.push_str("        </triangles>\n");

        xml.push_str("      </mesh>\n");
        let _ = writeln!(xml, "    </object>");
    }

    xml.push_str("  </resources>\n");

    // build — references each object
    xml.push_str("  <build>\n");
    for obj_id in &build_items {
        let _ = writeln!(xml, "    <item objectid=\"{}\" />", obj_id);
    }
    xml.push_str("  </build>\n");
    xml.push_str("</model>\n");

    xml
}

// ─── Minimal ZIP writer (STORE, no compression) ────────────────
//
// Avoids pulling in the `zip` crate and its transitive deps.
// 3MF files are small enough that DEFLATE gains are irrelevant
// compared to the WASM binary size savings.

struct ZipEntry {
    name: Vec<u8>,
    data: Vec<u8>,
    crc32: u32,
    offset: u32,
}

struct ZipWriter {
    entries: Vec<ZipEntry>,
    buf: Vec<u8>,
}

impl ZipWriter {
    fn new() -> Self {
        Self { entries: Vec::new(), buf: Vec::new() }
    }

    fn add(&mut self, name: &str, data: &[u8]) {
        let offset = self.buf.len() as u32;
        let crc = crc32(data);
        let name_bytes = name.as_bytes();
        let data_len = data.len() as u32;

        // Local file header
        self.buf.extend_from_slice(&0x04034b50u32.to_le_bytes()); // signature
        self.buf.extend_from_slice(&20u16.to_le_bytes());         // version needed
        self.buf.extend_from_slice(&0u16.to_le_bytes());          // flags
        self.buf.extend_from_slice(&0u16.to_le_bytes());          // compression: STORE
        self.buf.extend_from_slice(&0u16.to_le_bytes());          // mod time
        self.buf.extend_from_slice(&0u16.to_le_bytes());          // mod date
        self.buf.extend_from_slice(&crc.to_le_bytes());           // crc32
        self.buf.extend_from_slice(&data_len.to_le_bytes());      // compressed size
        self.buf.extend_from_slice(&data_len.to_le_bytes());      // uncompressed size
        self.buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());          // extra field len
        self.buf.extend_from_slice(name_bytes);
        self.buf.extend_from_slice(data);

        self.entries.push(ZipEntry {
            name: name_bytes.to_vec(),
            data: data.to_vec(),
            crc32: crc,
            offset,
        });
    }

    fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.buf.len() as u32;

        for entry in &self.entries {
            let data_len = entry.data.len() as u32;

            // Central directory entry
            self.buf.extend_from_slice(&0x02014b50u32.to_le_bytes()); // signature
            self.buf.extend_from_slice(&20u16.to_le_bytes());         // version made by
            self.buf.extend_from_slice(&20u16.to_le_bytes());         // version needed
            self.buf.extend_from_slice(&0u16.to_le_bytes());          // flags
            self.buf.extend_from_slice(&0u16.to_le_bytes());          // compression
            self.buf.extend_from_slice(&0u16.to_le_bytes());          // mod time
            self.buf.extend_from_slice(&0u16.to_le_bytes());          // mod date
            self.buf.extend_from_slice(&entry.crc32.to_le_bytes());
            self.buf.extend_from_slice(&data_len.to_le_bytes());      // compressed
            self.buf.extend_from_slice(&data_len.to_le_bytes());      // uncompressed
            self.buf.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
            self.buf.extend_from_slice(&0u16.to_le_bytes());          // extra len
            self.buf.extend_from_slice(&0u16.to_le_bytes());          // comment len
            self.buf.extend_from_slice(&0u16.to_le_bytes());          // disk number
            self.buf.extend_from_slice(&0u16.to_le_bytes());          // internal attrs
            self.buf.extend_from_slice(&0u32.to_le_bytes());          // external attrs
            self.buf.extend_from_slice(&entry.offset.to_le_bytes());
            self.buf.extend_from_slice(&entry.name);
        }

        let cd_size = self.buf.len() as u32 - cd_offset;
        let entry_count = self.entries.len() as u16;

        // End of central directory
        self.buf.extend_from_slice(&0x06054b50u32.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());          // disk number
        self.buf.extend_from_slice(&0u16.to_le_bytes());          // cd start disk
        self.buf.extend_from_slice(&entry_count.to_le_bytes());
        self.buf.extend_from_slice(&entry_count.to_le_bytes());
        self.buf.extend_from_slice(&cd_size.to_le_bytes());
        self.buf.extend_from_slice(&cd_offset.to_le_bytes());
        self.buf.extend_from_slice(&0u16.to_le_bytes());          // comment len

        self.buf
    }
}

// ─── CRC-32 (ISO 3309 / ITU-T V.42) ──────────────────────────

fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
        let mut t = [0u32; 256];
        for i in 0..256u32 {
            let mut c = i;
            for _ in 0..8 {
                if c & 1 != 0 {
                    c = 0xEDB88320 ^ (c >> 1);
                } else {
                    c >>= 1;
                }
            }
            t[i as usize] = c;
        }
        t
    });

    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc = TABLE[((crc ^ byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

// ─── tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn one_triangle() -> Mesh {
        Mesh {
            positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
            normals: vec![0.0; 9],
            indices: vec![0, 1, 2],
            classifications: vec![0; 3],
            contributors: vec![],
        }
    }

    fn two_triangles() -> Mesh {
        Mesh {
            positions: vec![
                0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0,
            ],
            normals: vec![0.0; 12],
            indices: vec![0, 1, 2, 0, 2, 3],
            classifications: vec![0; 4],
            contributors: vec![],
        }
    }

    #[test]
    fn zip_starts_with_local_header_signature() {
        let mesh = one_triangle();
        let bytes = export_3mf(&[&mesh]);
        assert_eq!(&bytes[0..4], &0x04034b50u32.to_le_bytes());
    }

    #[test]
    fn zip_ends_with_eocd_signature() {
        let mesh = one_triangle();
        let bytes = export_3mf(&[&mesh]);
        let len = bytes.len();
        assert!(len >= 22);
        let eocd_offset = len - 22;
        assert_eq!(
            &bytes[eocd_offset..eocd_offset + 4],
            &0x06054b50u32.to_le_bytes(),
        );
    }

    #[test]
    fn zip_contains_three_entries() {
        let mesh = one_triangle();
        let bytes = export_3mf(&[&mesh]);
        let len = bytes.len();
        let eocd_offset = len - 22;
        let entry_count =
            u16::from_le_bytes([bytes[eocd_offset + 8], bytes[eocd_offset + 9]]);
        assert_eq!(entry_count, 3);
    }

    #[test]
    fn model_xml_contains_vertex_and_triangle_data() {
        let mesh = one_triangle();
        let xml = model_xml(&[&mesh]);
        assert!(xml.contains("<vertex x=\"0.000000\""), "vertex missing: {xml}");
        assert!(xml.contains("<triangle v1=\"0\" v2=\"1\" v3=\"2\""), "triangle missing: {xml}");
    }

    #[test]
    fn model_xml_unit_is_millimeter() {
        let mesh = one_triangle();
        let xml = model_xml(&[&mesh]);
        assert!(xml.contains("unit=\"millimeter\""));
    }

    #[test]
    fn model_xml_has_basematerials_with_color() {
        let mesh = one_triangle();
        let xml = model_xml(&[&mesh]);
        assert!(xml.contains("displaycolor=\"#8CA6D9\"") || xml.contains("displaycolor=\"#8CA5D9\""),
            "body 0 color missing: {xml}");
    }

    #[test]
    fn multi_body_produces_multiple_objects() {
        let a = one_triangle();
        let b = two_triangles();
        let xml = model_xml(&[&a, &b]);
        assert_eq!(xml.matches("<object id=").count(), 2);
        assert_eq!(xml.matches("<item objectid=").count(), 2);
        assert!(xml.contains("pindex=\"0\""));
        assert!(xml.contains("pindex=\"1\""));
    }

    #[test]
    fn empty_mesh_is_skipped() {
        let empty = Mesh {
            positions: vec![],
            normals: vec![],
            indices: vec![],
            classifications: vec![],
            contributors: vec![],
        };
        let real = one_triangle();
        let xml = model_xml(&[&empty, &real]);
        assert_eq!(xml.matches("<object id=").count(), 1);
    }

    #[test]
    fn crc32_known_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF43926);
    }

    #[test]
    fn color_hex_body0_is_blue() {
        let hex = color_hex(0);
        assert_eq!(hex.len(), 7);
        assert!(hex.starts_with('#'));
    }

    #[test]
    fn color_hex_wraps_around_palette() {
        assert_eq!(color_hex(0), color_hex(BODY_COLORS.len()));
    }
}
