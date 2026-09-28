//! UV layout preview: per-mesh UV edges and checks, rasterized on a worker thread into an
//! image of just the visible part of UV space, so million-triangle meshes stay responsive.
//!
//! UVs are stored like glTF (v grows downward), which is also how images are laid out: the
//! layout is drawn with v down and the texture upright, the same picture Blender shows.

use crate::scene::{Image, Material, Mesh};

/// Base color previews for the UV background are capped at this size.
const TEXTURE_PREVIEW: u32 = 1024;

pub struct UvMesh {
    pub uvs: Vec<[f32; 2]>,
    /// Unique edges (the loader splits vertices at seams, so these are UV edges).
    pub edges: Vec<[u32; 2]>,
    /// Triangles whose UVs are mirrored relative to the surface.
    pub flipped: Vec<[u32; 3]>,
    /// Triangles with a corner outside the 0..1 square.
    pub outside: usize,
    /// Total surface in square meters (world space) and in UV space, for texel density.
    pub area_world: f64,
    pub area_uv: f64,
    pub material: usize,
}

pub struct UvData {
    /// `None` for meshes without UVs.
    pub meshes: Vec<Option<UvMesh>>,
    /// Base color preview per material: size and RGBA pixels.
    pub textures: Vec<Option<([usize; 2], Vec<u8>)>>,
}

pub fn extract(meshes: &[Mesh], materials: &[Material], images: &[Image]) -> UvData {
    let meshes = meshes
        .iter()
        .map(|m| {
            let uvs = m.uvs.clone()?;
            let mut flipped = Vec::new();
            let mut outside = 0;
            let (mut area_world, mut area_uv) = (0.0f64, 0.0f64);
            for t in m.indices.chunks_exact(3) {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| uvs[i as usize]);
                let [p, q, r] = [t[0], t[1], t[2]].map(|i| m.transform.transform_point3(glam::Vec3::from(m.positions[i as usize])));
                area_world += (q - p).cross(r - p).length() as f64 * 0.5;
                area_uv += signed_area(a, b, c).abs() as f64 * 0.5;
                if signed_area(a, b, c) > 0.0 {
                    flipped.push([t[0], t[1], t[2]]);
                }
                let out = |p: [f32; 2]| p.iter().any(|&x| !(-1e-4..=1.0 + 1e-4).contains(&x));
                if out(a) || out(b) || out(c) {
                    outside += 1;
                }
            }
            Some(UvMesh {
                edges: m.edges.chunks_exact(2).map(|e| [e[0], e[1]]).collect(),
                flipped,
                outside,
                area_world,
                area_uv,
                material: m.material,
                uvs,
            })
        })
        .collect();
    let textures = materials
        .iter()
        .map(|mat| {
            let image = images.get(mat.base_color_tex?)?;
            (!image.mips.is_empty() && image.width > 0).then(|| image.thumbnail(TEXTURE_PREVIEW, None))
        })
        .collect();
    UvData { meshes, textures }
}

impl UvMesh {
    /// Texture pixels per meter for a `texture_size`-pixel square texture.
    pub fn texel_density(&self, texture_size: f64) -> Option<f64> {
        (self.area_world > 1e-12).then(|| (self.area_uv / self.area_world).sqrt() * texture_size)
    }
}

/// Twice the signed area in stored (v down) coordinates. A triangle wound counter-clockwise
/// in 3D, with its texture upright, comes out negative; positive means mirrored UVs.
fn signed_area(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// Visible part of UV space: min and max corners (u, v).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Window {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

/// Draws the UV edges (white, brighter where dense) and mirrored faces (red) of `meshes`
/// as seen through `window`, into a `size` RGBA image.
pub fn rasterize(meshes: &[&UvMesh], window: Window, size: [usize; 2]) -> Vec<u8> {
    let [w, h] = size;
    let mut edge = vec![0u8; w * h];
    let mut fill = vec![false; w * h];
    let sx = w as f32 / (window.max[0] - window.min[0]);
    let sy = h as f32 / (window.max[1] - window.min[1]);
    let to_px = |p: [f32; 2]| [(p[0] - window.min[0]) * sx, (p[1] - window.min[1]) * sy];
    for mesh in meshes {
        for tri in &mesh.flipped {
            let [a, b, c] = tri.map(|i| to_px(mesh.uvs[i as usize]));
            fill_triangle(&mut fill, size, a, b, c);
        }
        for &[i, j] in &mesh.edges {
            let (a, b) = (to_px(mesh.uvs[i as usize]), to_px(mesh.uvs[j as usize]));
            line(&mut edge, size, a, b);
        }
    }
    let mut out = vec![0u8; w * h * 4];
    for k in 0..w * h {
        let px = &mut out[k * 4..k * 4 + 4];
        if edge[k] > 0 {
            let v = edge[k];
            px.copy_from_slice(&[235, 235, 235, v]);
        } else if fill[k] {
            px.copy_from_slice(&[229, 72, 77, 110]);
        }
    }
    out
}

/// One-pixel line with coverage piling up where edges are dense.
fn line(buf: &mut [u8], [w, h]: [usize; 2], a: [f32; 2], b: [f32; 2]) {
    let (wf, hf) = (w as f32, h as f32);
    if a[0].max(b[0]) < 0.0 || a[0].min(b[0]) >= wf || a[1].max(b[1]) < 0.0 || a[1].min(b[1]) >= hf {
        return;
    }
    let steps = (b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil().clamp(1.0, 16384.0) as usize;
    for s in 0..=steps {
        let t = s as f32 / steps as f32;
        let x = a[0] + (b[0] - a[0]) * t;
        let y = a[1] + (b[1] - a[1]) * t;
        if x >= 0.0 && y >= 0.0 && x < wf && y < hf {
            let p = &mut buf[y as usize * w + x as usize];
            *p = p.saturating_add(if *p == 0 { 150 } else { 35 });
        }
    }
}

fn fill_triangle(buf: &mut [bool], [w, h]: [usize; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) {
    let x0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
    let y0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
    let x1 = (a[0].max(b[0]).max(c[0]).ceil().max(0.0) as usize).min(w);
    let y1 = (a[1].max(b[1]).max(c[1]).ceil().max(0.0) as usize).min(h);
    let area = signed_area(a, b, c);
    if area.abs() < 1e-12 {
        return;
    }
    for y in y0..y1 {
        for x in x0..x1 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let (e0, e1, e2) = (signed_area(a, b, p), signed_area(b, c, p), signed_area(c, a, p));
            let inside = if area > 0.0 { e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0 } else { e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0 };
            if inside {
                buf[y * w + x] = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{MeshData, MeshRig};
    use glam::Mat4;

    /// A quad facing +Z, counter-clockwise, mapped upright like a glTF exporter would.
    fn quad(mirror: bool) -> Mesh {
        let u = |x: f32| if mirror { 1.0 - x } else { x };
        Mesh::build(
            MeshData {
                name: "quad".into(),
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
                normals: None,
                uvs: Some(vec![[u(0.0), 1.0], [u(1.0), 1.0], [u(1.0), 0.0], [u(0.0), 0.0]]),
                tangents: None,
                colors: None,
                indices: vec![0, 1, 2, 0, 2, 3],
                transform: Mat4::IDENTITY,
                material: 0,
                rig: MeshRig::default(),
            },
            false,
        )
    }

    #[test]
    fn mirrored_uvs_are_flagged() {
        let data = extract(&[quad(false), quad(true)], &[], &[]);
        let [a, b] = [&data.meshes[0], &data.meshes[1]].map(|m| m.as_ref().unwrap());
        assert!(a.flipped.is_empty());
        assert_eq!(b.flipped.len(), 2);
        assert_eq!(a.edges.len(), 5);
        assert_eq!(a.outside, 0);
        // 1 m² mapped on the whole 0..1 square: density equals the texture size per meter.
        assert!((a.area_world - 1.0).abs() < 1e-6 && (a.area_uv - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rasterizes_edges_inside_the_window() {
        let data = extract(&[quad(false)], &[], &[]);
        let mesh = data.meshes[0].as_ref().unwrap();
        let img = rasterize(&[mesh], Window { min: [0.0, 0.0], max: [1.0, 1.0] }, [64, 64]);
        let alpha = |x: usize, y: usize| img[(y * 64 + x) * 4 + 3];
        assert!(alpha(32, 0) > 0, "top border");
        assert_eq!(alpha(10, 40), 0, "inside the lower-left triangle stays empty");
    }
}
