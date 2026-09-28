//! PLY (Stanford polygon file): ASCII and binary, little or big endian. Reads positions,
//! normals, UVs (s/t, u/v, texture_u/texture_v) and vertex colors, plus polygon faces
//! (fan-triangulated). Other elements are skipped. No units: read as meters, Z up.

use std::path::Path;

use glam::Mat4;

use crate::scene::{Material, Mesh, MeshData, MeshRig, Scene};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Kind {
    fn parse(name: &str) -> Result<Kind, String> {
        Ok(match name {
            "char" | "int8" => Kind::I8,
            "uchar" | "uint8" => Kind::U8,
            "short" | "int16" => Kind::I16,
            "ushort" | "uint16" => Kind::U16,
            "int" | "int32" => Kind::I32,
            "uint" | "uint32" => Kind::U32,
            "float" | "float32" => Kind::F32,
            "double" | "float64" => Kind::F64,
            other => return Err(format!("Unknown PLY property type \"{other}\"")),
        })
    }

    fn size(self) -> usize {
        match self {
            Kind::I8 | Kind::U8 => 1,
            Kind::I16 | Kind::U16 => 2,
            Kind::I32 | Kind::U32 | Kind::F32 => 4,
            Kind::F64 => 8,
        }
    }

    /// Full-scale value for normalizing integer colors.
    fn color_scale(self) -> f64 {
        match self {
            Kind::U8 | Kind::I8 => 255.0,
            Kind::U16 | Kind::I16 => 65535.0,
            _ => 1.0,
        }
    }
}

struct Property {
    name: String,
    kind: Kind,
    /// `Some(count type)` for list properties.
    list: Option<Kind>,
}

struct Element {
    name: String,
    count: usize,
    props: Vec<Property>,
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Ascii,
    Little,
    Big,
}

/// Reads values one at a time from the body, whatever the encoding.
struct Reader<'a> {
    format: Format,
    bytes: &'a [u8],
    pos: usize,
    tokens: std::str::SplitAsciiWhitespace<'a>,
}

impl Reader<'_> {
    fn read(&mut self, kind: Kind) -> Result<f64, String> {
        if self.format == Format::Ascii {
            let t = self.tokens.next().ok_or("The PLY file ends early")?;
            return t.parse::<f64>().map_err(|_| format!("Invalid number \"{t}\" in the PLY file"));
        }
        let n = kind.size();
        let b = self.bytes.get(self.pos..self.pos + n).ok_or("The PLY file ends early")?;
        self.pos += n;
        let mut a = [0u8; 8];
        a[..n].copy_from_slice(b);
        if self.format == Format::Big {
            a[..n].reverse();
        }
        Ok(match kind {
            Kind::I8 => a[0] as i8 as f64,
            Kind::U8 => a[0] as f64,
            Kind::I16 => i16::from_le_bytes([a[0], a[1]]) as f64,
            Kind::U16 => u16::from_le_bytes([a[0], a[1]]) as f64,
            Kind::I32 => i32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Kind::U32 => u32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Kind::F32 => f32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
            Kind::F64 => f64::from_le_bytes(a),
        })
    }
}

pub fn load(path: &Path) -> Result<Scene, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    parse(&bytes, &file_stem(path))
}

fn file_stem(path: &Path) -> String {
    path.file_stem().map_or("PLY".into(), |s| s.to_string_lossy().into_owned())
}

pub fn parse(bytes: &[u8], name: &str) -> Result<Scene, String> {
    // Header: text lines up to "end_header".
    let end = find(bytes, b"end_header").ok_or("Not a PLY file (no end_header)")?;
    let header = std::str::from_utf8(&bytes[..end]).map_err(|_| "The PLY header isn't text")?;
    let mut body_start = end + b"end_header".len();
    // The body starts after the header's line break (\n or \r\n).
    while body_start < bytes.len() && (bytes[body_start] == b'\r' || bytes[body_start] == b'\n') {
        let nl = bytes[body_start] == b'\n';
        body_start += 1;
        if nl {
            break;
        }
    }
    let mut lines = header.lines().map(str::trim);
    if lines.next() != Some("ply") {
        return Err("Not a PLY file".into());
    }
    let mut format = None;
    let mut elements: Vec<Element> = Vec::new();
    for line in lines {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["format", f, ..] => {
                format = Some(match *f {
                    "ascii" => Format::Ascii,
                    "binary_little_endian" => Format::Little,
                    "binary_big_endian" => Format::Big,
                    other => return Err(format!("Unknown PLY format \"{other}\"")),
                })
            }
            ["element", name, count] => elements.push(Element {
                name: name.to_string(),
                count: count.parse().map_err(|_| "Invalid PLY element count")?,
                props: Vec::new(),
            }),
            ["property", "list", count, item, name] => {
                let e = elements.last_mut().ok_or("PLY property before any element")?;
                e.props.push(Property { name: name.to_string(), kind: Kind::parse(item)?, list: Some(Kind::parse(count)?) });
            }
            ["property", kind, name] => {
                let e = elements.last_mut().ok_or("PLY property before any element")?;
                e.props.push(Property { name: name.to_string(), kind: Kind::parse(kind)?, list: None });
            }
            _ => {}
        }
    }
    let format = format.ok_or("The PLY header has no format line")?;
    let body = &bytes[body_start.min(bytes.len())..];
    let text = if format == Format::Ascii { std::str::from_utf8(body).map_err(|_| "The PLY body isn't text")? } else { "" };
    let mut r = Reader { format, bytes: body, pos: 0, tokens: text.split_ascii_whitespace() };

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for e in &elements {
        let find = |names: &[&str]| e.props.iter().position(|p| p.list.is_none() && names.contains(&p.name.as_str()));
        let (px, py, pz) = (find(&["x"]), find(&["y"]), find(&["z"]));
        let (nx, ny, nz) = (find(&["nx"]), find(&["ny"]), find(&["nz"]));
        let (tu, tv) = (find(&["s", "u", "texture_u", "texture_s"]), find(&["t", "v", "texture_v", "texture_t"]));
        let (cr, cg, cb, ca) = (find(&["red", "r", "diffuse_red"]), find(&["green", "g", "diffuse_green"]), find(&["blue", "b", "diffuse_blue"]), find(&["alpha", "a"]));
        let face_list = e.props.iter().position(|p| p.list.is_some() && (p.name == "vertex_indices" || p.name == "vertex_index"));
        let is_vertex = e.name == "vertex";
        let is_face = e.name == "face" && face_list.is_some();
        let mut values = vec![0.0f64; e.props.len()];
        let mut polygon: Vec<u32> = Vec::new();
        for _ in 0..e.count {
            for (k, p) in e.props.iter().enumerate() {
                match p.list {
                    None => values[k] = r.read(p.kind)?,
                    Some(count_kind) => {
                        let n = r.read(count_kind)? as usize;
                        let keep = is_face && Some(k) == face_list;
                        polygon.clear();
                        for _ in 0..n {
                            let v = r.read(p.kind)?;
                            if keep {
                                polygon.push(v as u32);
                            }
                        }
                        if keep {
                            for i in 1..polygon.len().saturating_sub(1) {
                                indices.extend_from_slice(&[polygon[0], polygon[i], polygon[i + 1]]);
                            }
                        }
                    }
                }
            }
            if is_vertex {
                let get = |i: Option<usize>| i.map_or(0.0, |i| values[i]) as f32;
                positions.push([get(px), get(py), get(pz)]);
                if nx.is_some() {
                    normals.push([get(nx), get(ny), get(nz)]);
                }
                if tu.is_some() && tv.is_some() {
                    // PLY UVs are v-up (like OBJ); the viewer stores them v-down.
                    uvs.push([get(tu), 1.0 - get(tv)]);
                }
                if cr.is_some() {
                    let c = |i: Option<usize>, default: f64| {
                        i.map_or(default, |i| values[i] / e.props[i].kind.color_scale()) as f32
                    };
                    let srgb = [c(cr, 0.0), c(cg, 0.0), c(cb, 0.0)].map(crate::render::srgb_channel_to_linear);
                    colors.push([srgb[0], srgb[1], srgb[2], c(ca, 1.0)]);
                }
            }
        }
    }
    if positions.is_empty() {
        return Err("The PLY file has no vertices".into());
    }
    let count = positions.len() as u32;
    if let Some(bad) = indices.iter().find(|&&i| i >= count) {
        return Err(format!("The PLY file references vertex {bad}, but has only {count}"));
    }
    let n = positions.len();
    let mesh = Mesh::build(
        MeshData {
            name: name.to_string(),
            normals: (normals.len() == n).then_some(normals),
            uvs: (uvs.len() == n).then_some(uvs),
            colors: (colors.len() == n).then_some(colors),
            tangents: None,
            positions,
            indices,
            transform: Mat4::IDENTITY,
            material: 0,
            rig: MeshRig::default(),
        },
        false,
    );
    Ok(Scene::new(vec![mesh], vec![Material::default()], Vec::new(), n))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASCII: &str = "ply\nformat ascii 1.0\ncomment test\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\n\
        property uchar red\nproperty uchar green\nproperty uchar blue\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n\
        0 0 0 255 0 0\n1 0 0 0 255 0\n1 1 0 0 0 255\n0 1 0 255 255 255\n4 0 1 2 3\n";

    #[test]
    fn ascii_quad_with_colors() {
        let scene = parse(ASCII.as_bytes(), "quad").unwrap();
        let m = &scene.meshes[0];
        assert_eq!(m.positions.len(), 4);
        assert_eq!(m.indices, vec![0, 1, 2, 0, 2, 3]);
        let c = m.colors.as_ref().unwrap();
        assert!((c[0][0] - 1.0).abs() < 1e-6 && c[0][1] == 0.0);
    }

    #[test]
    fn binary_matches_ascii() {
        let mut bytes = b"ply\r\nformat binary_little_endian 1.0\r\nelement vertex 3\r\nproperty float x\r\nproperty float y\r\nproperty float z\r\n\
element face 1\r\nproperty list uchar uint vertex_indices\r\nend_header\r\n"
            .to_vec();
        for v in [[0.0f32, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 3.0, 0.0]] {
            for c in v {
                bytes.extend_from_slice(&c.to_le_bytes());
            }
        }
        bytes.push(3);
        for i in [0u32, 1, 2] {
            bytes.extend_from_slice(&i.to_le_bytes());
        }
        let scene = parse(&bytes, "tri").unwrap();
        assert_eq!(scene.meshes[0].positions[2], [0.0, 3.0, 0.0]);
        assert_eq!(scene.meshes[0].indices, vec![0, 1, 2]);
    }
}
