//! Software thumbnail renderer.
//!
//! Runs on the CPU only: no GPU, no window and no helper process, so it works inside Explorer's
//! isolated thumbnail process on any drive. The look follows Blender's Solid mode (studio lights
//! fixed to the camera, base color, textures and vertex colors) from a 3/4 view fitted tightly to
//! the model, on a transparent background.

use std::f32::consts::PI;

use glam::{Mat3, Vec2, Vec3, Vec4};

use crate::color::srgb_channel_to_linear;
use crate::scene::{AlphaMode, Image, Material, Scene};

/// Rendered at this multiple of the requested size, then box-filtered down for clean edges.
const SUPERSAMPLE: u32 = 2;
const FOV_Y: f32 = 30.0 * PI / 180.0;
/// Share of the frame the model's larger side fills.
const FILL: f32 = 0.86;

/// Renders `scene` into a `size` x `size` straight-alpha RGBA image, or `None` when there is
/// nothing to draw.
pub fn render(scene: &Scene, size: u32) -> Option<Vec<u8>> {
    let size = size.clamp(16, 1024);
    let res = (size * SUPERSAMPLE).min(2048);
    let points = sample_points(scene);
    if points.is_empty() || !scene.bounds.is_valid() {
        return None;
    }
    let camera = Camera::framing(scene, &points);
    let mut target = Target::new(res);
    let srgb = SrgbTable::new();
    for mesh in &scene.meshes {
        let material = scene.materials.get(mesh.material);
        draw_mesh(&mut target, &camera, mesh, material, &scene.images, &srgb);
    }
    if !target.depth.iter().any(|d| d.is_finite()) {
        return None;
    }
    Some(downsample(&target.resolve(), res, size))
}

/// Pinhole camera looking at the model; world is Z-up like the viewport.
struct Camera {
    eye: Vec3,
    right: Vec3,
    up: Vec3,
    back: Vec3,
    tan_half: f32,
}

impl Camera {
    fn new(target: Vec3, distance: f32, yaw: f32, pitch: f32) -> Self {
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        let back = Vec3::new(cp * cy, cp * sy, sp);
        let right = Vec3::new(-sy, cy, 0.0);
        let up = back.cross(right);
        Self { eye: target + back * distance, right, up, back, tan_half: (FOV_Y * 0.5).tan() }
    }

    /// Normalized device coordinates (x right, y up) and the distance in front of the camera.
    fn project(&self, p: Vec3) -> Vec3 {
        let d = p - self.eye;
        let z = (-d.dot(self.back)).max(1e-9);
        Vec3::new(d.dot(self.right) / (z * self.tan_half), d.dot(self.up) / (z * self.tan_half), z)
    }

    /// Same framing as the viewport's thumbnails: Blender's 3/4 view, flatter from above and tall
    /// things closer to eye level, then fitted to the sampled geometry.
    fn framing(scene: &Scene, points: &[Vec3]) -> Self {
        let bounds = &scene.bounds;
        let size = bounds.size();
        let footprint = size.x.max(size.y).max(1e-6);
        let yaw = (-60f32).to_radians();
        let pitch = if size.z < footprint * 0.15 {
            55f32
        } else if size.z > footprint * 3.0 {
            12f32
        } else {
            25f32
        }
        .to_radians();
        let mut target = bounds.center();
        let mut distance = bounds.radius().max(1e-4) / (FOV_Y * 0.5).sin() * 0.92;
        for _ in 0..6 {
            let cam = Self::new(target, distance, yaw, pitch);
            let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for p in points {
                let ndc = cam.project(*p).truncate();
                lo = lo.min(ndc);
                hi = hi.max(ndc);
            }
            // Center the projected box, then scale the distance so its larger side fills FILL.
            let center = (lo + hi) * 0.5;
            let half_h = distance * cam.tan_half;
            target += (cam.right * center.x + cam.up * center.y) * half_h;
            let extent = ((hi - lo) * 0.5).max_element();
            distance *= (extent / FILL).clamp(0.2, 5.0);
        }
        Self::new(target, distance, yaw, pitch)
    }
}

/// Up to ~60k world-space vertex positions spread over every mesh.
fn sample_points(scene: &Scene) -> Vec<Vec3> {
    let total: usize = scene.meshes.iter().map(|m| m.positions.len()).sum();
    let stride = (total / 60_000).max(1);
    scene
        .meshes
        .iter()
        .flat_map(|m| m.positions.iter().step_by(stride).map(move |p| m.transform.transform_point3(Vec3::from(*p))))
        .collect()
}

/// Color (linear, alpha) and depth buffers.
struct Target {
    res: u32,
    color: Vec<Vec4>,
    depth: Vec<f32>,
}

impl Target {
    fn new(res: u32) -> Self {
        let n = (res * res) as usize;
        Self { res, color: vec![Vec4::ZERO; n], depth: vec![f32::INFINITY; n] }
    }

    /// sRGB-encoded straight-alpha RGBA8.
    fn resolve(&self) -> Vec<u8> {
        let encode = |c: f32| {
            let c = c.clamp(0.0, 1.0);
            let s = if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
            (s * 255.0).round() as u8
        };
        self.color
            .iter()
            .flat_map(|c| [encode(c.x), encode(c.y), encode(c.z), (c.w.clamp(0.0, 1.0) * 255.0).round() as u8])
            .collect()
    }
}

/// sRGB byte to linear float.
struct SrgbTable([f32; 256]);

impl SrgbTable {
    fn new() -> Self {
        Self(std::array::from_fn(|i| srgb_channel_to_linear(i as f32 / 255.0)))
    }
}

/// A projected vertex.
#[derive(Clone, Copy)]
struct Vertex {
    /// Pixel position and distance in front of the camera.
    screen: Vec3,
    world: Vec3,
    normal: Vec3,
}

fn draw_mesh(
    target: &mut Target,
    camera: &Camera,
    mesh: &crate::scene::Mesh,
    material: Option<&Material>,
    images: &[Image],
    srgb: &SrgbTable,
) {
    let res = target.res as f32;
    let normal_matrix = Mat3::from_mat4(mesh.transform).inverse().transpose();
    let verts: Vec<Vertex> = mesh
        .positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let world = mesh.transform.transform_point3(Vec3::from(*p));
            let ndc = camera.project(world);
            let normal = mesh.normals.get(i).map_or(Vec3::ZERO, |n| (normal_matrix * Vec3::from(*n)).normalize_or_zero());
            Vertex { screen: Vec3::new((ndc.x * 0.5 + 0.5) * res, (0.5 - ndc.y * 0.5) * res, ndc.z), world, normal }
        })
        .collect();

    let base_factor = material.map_or(Vec4::splat(0.8), |m| Vec4::from(m.base_color));
    let texture = material
        .and_then(|m| m.base_color_tex)
        .and_then(|t| images.get(t))
        .filter(|_| mesh.uvs.is_some());
    let cutoff = match material.map(|m| m.alpha_mode) {
        Some(AlphaMode::Mask(c)) => Some(c),
        _ => None,
    };
    let emissive = material
        .filter(|m| m.emissive_tex.is_none())
        .map_or(Vec3::ZERO, |m| Vec3::from(m.emissive));
    // Anything closer than this is behind (or at) the lens.
    let near = 1e-6;

    for tri in mesh.indices.chunks_exact(3) {
        let [ia, ib, ic] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        let (Some(a), Some(b), Some(c)) = (verts.get(ia), verts.get(ib), verts.get(ic)) else { continue };
        if a.screen.z <= near || b.screen.z <= near || c.screen.z <= near {
            continue;
        }
        let area = edge(a.screen.truncate(), b.screen.truncate(), c.screen.truncate());
        if area.abs() < 1e-12 {
            continue;
        }
        let min = a.screen.min(b.screen).min(c.screen);
        let max = a.screen.max(b.screen).max(c.screen);
        if max.x < 0.0 || max.y < 0.0 || min.x >= res || min.y >= res {
            continue;
        }
        let x0 = min.x.floor().max(0.0) as u32;
        let y0 = min.y.floor().max(0.0) as u32;
        let x1 = (max.x.ceil() as u32).min(target.res - 1);
        let y1 = (max.y.ceil() as u32).min(target.res - 1);

        let face = (b.world - a.world).cross(c.world - a.world).normalize_or_zero();
        let uv = mesh.uvs.as_ref().map(|uvs| [ia, ib, ic].map(|i| uvs.get(i).map_or(Vec2::ZERO, |t| Vec2::from(*t))));
        let colors = mesh.colors.as_ref().map(|cs| [ia, ib, ic].map(|i| cs.get(i).map_or(Vec4::ONE, |c| Vec4::from(*c))));
        // One mip level per triangle, from how many texels land on each pixel.
        let lod = match (texture, uv) {
            (Some(img), Some(t)) => {
                let texel_area = ((t[1] - t[0]).perp_dot(t[2] - t[0]) * 0.5).abs() * (img.width * img.height) as f32;
                let pixel_area = (area * 0.5).abs().max(1e-6);
                (0.5 * (texel_area / pixel_area).max(1.0).log2()).max(0.0)
            }
            _ => 0.0,
        };
        let inv_z = [1.0 / a.screen.z, 1.0 / b.screen.z, 1.0 / c.screen.z];

        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w = [
                    edge(b.screen.truncate(), c.screen.truncate(), p) / area,
                    edge(c.screen.truncate(), a.screen.truncate(), p) / area,
                    edge(a.screen.truncate(), b.screen.truncate(), p) / area,
                ];
                if w[0] < 0.0 || w[1] < 0.0 || w[2] < 0.0 {
                    continue;
                }
                // Perspective-correct weights.
                let iz = w[0] * inv_z[0] + w[1] * inv_z[1] + w[2] * inv_z[2];
                let z = 1.0 / iz;
                let i = (y * target.res + x) as usize;
                if z >= target.depth[i] {
                    continue;
                }
                let pw = [w[0] * inv_z[0] * z, w[1] * inv_z[1] * z, w[2] * inv_z[2] * z];

                let mut base = base_factor;
                if let (Some(img), Some(t)) = (texture, uv) {
                    let st = t[0] * pw[0] + t[1] * pw[1] + t[2] * pw[2];
                    base *= sample(img, st, lod, srgb);
                }
                if let Some(cs) = colors {
                    base *= cs[0] * pw[0] + cs[1] * pw[1] + cs[2] * pw[2];
                }
                if cutoff.is_some_and(|c| base.w < c) {
                    continue;
                }

                let world = a.world * pw[0] + b.world * pw[1] + c.world * pw[2];
                let mut n = (a.normal * pw[0] + b.normal * pw[1] + c.normal * pw[2]).normalize_or_zero();
                if n == Vec3::ZERO {
                    n = face;
                }
                // Double-sided, like Blender: back faces are lit as if they faced the camera.
                if n.dot(camera.eye - world) < 0.0 {
                    n = -n;
                }
                let n_view = Vec3::new(n.dot(camera.right), n.dot(camera.up), n.dot(camera.back));
                target.depth[i] = z;
                target.color[i] = (studio(n_view, base.truncate()) + emissive).extend(1.0);
            }
        }
    }
}

fn edge(a: Vec2, b: Vec2, p: Vec2) -> f32 {
    (b - a).perp_dot(p - a)
}

/// The viewport's Solid-mode studio lights (fixed to the camera, not the world).
fn studio(n_view: Vec3, base: Vec3) -> Vec3 {
    let key = Vec3::new(-0.45, 0.65, 0.62).normalize();
    let fill = Vec3::new(0.75, -0.15, 0.45).normalize();
    let rim = Vec3::new(0.1, 0.45, -0.9).normalize();
    let wrap_key = ((n_view.dot(key) + 0.25) / 1.25).clamp(0.0, 1.0);
    let diffuse = 0.68 * wrap_key + 0.24 * n_view.dot(fill).max(0.0) + 0.18 * n_view.dot(rim).max(0.0);
    let ambient = 0.10 + (0.24 - 0.10) * (n_view.y * 0.5 + 0.5);
    let h = (key + Vec3::Z).normalize();
    let spec = n_view.dot(h).max(0.0).powf(48.0) * 0.18;
    base * (ambient + diffuse) + Vec3::splat(spec)
}

/// Bilinear sample (wrapping) from the mip level nearest to `lod`; linear RGB, straight alpha.
fn sample(img: &Image, uv: Vec2, lod: f32, srgb: &SrgbTable) -> Vec4 {
    let level = (lod.round() as usize).min(img.mips.len().saturating_sub(1));
    let Some(data) = img.mips.get(level) else { return Vec4::ONE };
    let w = (img.width >> level).max(1) as usize;
    let h = (img.height >> level).max(1) as usize;
    if data.len() < w * h * 4 {
        return Vec4::ONE;
    }
    let x = uv.x.rem_euclid(1.0) * w as f32 - 0.5;
    let y = uv.y.rem_euclid(1.0) * h as f32 - 0.5;
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let x0 = (x.floor() as isize).rem_euclid(w as isize) as usize;
    let y0 = (y.floor() as isize).rem_euclid(h as isize) as usize;
    let (x1, y1) = ((x0 + 1) % w, (y0 + 1) % h);
    let texel = |x: usize, y: usize| {
        let o = (y * w + x) * 4;
        Vec4::new(srgb.0[data[o] as usize], srgb.0[data[o + 1] as usize], srgb.0[data[o + 2] as usize], data[o + 3] as f32 / 255.0)
    };
    let top = texel(x0, y0).lerp(texel(x1, y0), fx);
    let bottom = texel(x0, y1).lerp(texel(x1, y1), fx);
    top.lerp(bottom, fy)
}

/// Box filter from `res` x `res` to `size` x `size` (premultiplied so edges don't darken).
fn downsample(src: &[u8], res: u32, size: u32) -> Vec<u8> {
    let (res, size) = (res as usize, size as usize);
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        let (y0, y1) = (y * res / size, ((y + 1) * res / size).max(y * res / size + 1));
        for x in 0..size {
            let (x0, x1) = (x * res / size, ((x + 1) * res / size).max(x * res / size + 1));
            let mut acc = [0f32; 4];
            let mut n = 0f32;
            for sy in y0..y1.min(res) {
                for sx in x0..x1.min(res) {
                    let p = &src[(sy * res + sx) * 4..(sy * res + sx) * 4 + 4];
                    let a = p[3] as f32 / 255.0;
                    acc[0] += p[0] as f32 * a;
                    acc[1] += p[1] as f32 * a;
                    acc[2] += p[2] as f32 * a;
                    acc[3] += a;
                    n += 1.0;
                }
            }
            let o = (y * size + x) * 4;
            if acc[3] > 0.0 {
                for c in 0..3 {
                    out[o + c] = (acc[c] / acc[3]).round().clamp(0.0, 255.0) as u8;
                }
            }
            out[o + 3] = (acc[3] / n.max(1.0) * 255.0).round() as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Aabb, Mesh, MeshRig};
    use glam::Mat4;

    fn quad_scene() -> Scene {
        let positions = vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]];
        let mesh = Mesh {
            name: "quad".into(),
            normals: vec![[0.0, 0.0, 1.0]; 4],
            positions,
            uvs: None,
            tangents: None,
            colors: None,
            indices: vec![0, 1, 2, 0, 2, 3],
            edges: Vec::new(),
            transform: Mat4::IDENTITY,
            material: 0,
            bounds: Aabb { min: Vec3::new(-1.0, -1.0, 0.0), max: Vec3::new(1.0, 1.0, 0.0) },
            rig: MeshRig::default(),
        };
        Scene::new(vec![mesh], Vec::new(), Vec::new(), 4)
    }

    #[test]
    fn renders_opaque_model_on_transparent_background() {
        let img = render(&quad_scene(), 64).expect("rendered");
        assert_eq!(img.len(), 64 * 64 * 4);
        let alpha = |x: usize, y: usize| img[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha(32, 32), 255, "model covers the center");
        assert_eq!(alpha(0, 0), 0, "corners stay transparent");
    }

    #[test]
    fn empty_scene_has_no_thumbnail() {
        assert!(render(&Scene::new(Vec::new(), Vec::new(), Vec::new(), 0), 64).is_none());
    }
}
