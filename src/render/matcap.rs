//! MatCaps: most are generated (each preset shades a hemisphere of view-space normals), two
//! come from Blender's CC0 set (`assets/matcap`). Like Blender's, a MatCap has a diffuse part,
//! multiplied by the object color, and a specular part added on top; generated ones only have
//! the diffuse part. Both are stored as sRGB bytes, side by side in one GPU texture.

use glam::Vec3;

pub const SIZE: usize = 256;

#[derive(Clone, Copy)]
pub struct Preset {
    pub name: &'static str,
    source: Source,
}

#[derive(Clone, Copy)]
enum Source {
    Shade(fn(n: Vec3) -> Vec3),
    /// A Blender MatCap: multilayer EXR with `diffuse.*` and `specular.*` channels.
    Exr(&'static [u8]),
}

const fn shade(name: &'static str, f: fn(Vec3) -> Vec3) -> Preset {
    Preset { name, source: Source::Shade(f) }
}

// New presets go at the end: settings store the index.
pub const PRESETS: &[Preset] = &[
    shade("Clay", clay),
    shade("Clay Dark", clay_dark),
    shade("Studio Gloss", gloss),
    shade("Chrome", chrome),
    shade("Red Wax", red_wax),
    shade("Jade", jade),
    shade("Skin", skin),
    shade("Toon", toon),
    shade("Rim Light", rim_light),
    shade("Normals", normals),
    Preset { name: "Clay Warm", source: Source::Exr(include_bytes!("../../assets/matcap/clay_warm.exr")) },
    Preset { name: "Basic Bright", source: Source::Exr(include_bytes!("../../assets/matcap/basic_bright.exr")) },
];

/// Object color the thumbnails show image MatCaps on (Blender's default material gray).
const PREVIEW_BASE: f32 = 0.8;

/// A preset's diffuse and specular parts, SIZE*SIZE sRGB RGBA8 pixels each.
pub struct Matcap {
    pub diffuse: Vec<u8>,
    pub specular: Vec<u8>,
}

pub fn generate(index: usize) -> Matcap {
    let preset = PRESETS[index.min(PRESETS.len() - 1)];
    let black = vec![0u8; SIZE * SIZE * 4];
    match preset.source {
        Source::Shade(f) => Matcap { diffuse: bake(&f), specular: black },
        Source::Exr(bytes) => match read_exr(bytes) {
            Some([d, s]) => Matcap { diffuse: bake(&|n| d.sample(n)), specular: bake(&|n| s.sample(n)) },
            None => Matcap { diffuse: bake(&clay), specular: black },
        },
    }
}

/// What the picker shows: the MatCap on a plain gray object.
pub fn preview(index: usize) -> Vec<u8> {
    let image = matches!(PRESETS[index.min(PRESETS.len() - 1)].source, Source::Exr(_));
    let m = generate(index);
    let mut out = m.diffuse;
    if image {
        for (d, s) in out.chunks_exact_mut(4).zip(m.specular.chunks_exact(4)) {
            for ch in 0..3 {
                d[ch] = linear_to_srgb_u8(srgb_u8_to_linear(d[ch]) * PREVIEW_BASE + srgb_u8_to_linear(s[ch]));
            }
        }
    }
    out
}

fn bake(shade: &dyn Fn(Vec3) -> Vec3) -> Vec<u8> {
    let mut out = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let u = (x as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
            let v = -((y as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0);
            // Clamp outside the disc to its rim so edge samples never pick up garbage (or an
            // image's anti-aliased border).
            let r2 = u * u + v * v;
            let (u, v) = if r2 > 0.98 {
                let s = 0.98f32.sqrt() / r2.sqrt();
                (u * s, v * s)
            } else {
                (u, v)
            };
            let n = Vec3::new(u, v, (1.0 - u * u - v * v).max(0.0).sqrt());
            let c = shade(n);
            for ch in c.to_array() {
                out.push(linear_to_srgb_u8(ch));
            }
            out.push(255);
        }
    }
    out
}

/// One layer of an image MatCap: linear RGB, rows top to bottom, the disc filling the square.
struct Layer {
    width: usize,
    height: usize,
    rgb: Vec<Vec3>,
}

impl Layer {
    /// Bilinear sample where the view-space normal `n` points.
    fn sample(&self, n: Vec3) -> Vec3 {
        let x = ((n.x * 0.5 + 0.5) * self.width as f32 - 0.5).clamp(0.0, (self.width - 1) as f32);
        let y = ((0.5 - n.y * 0.5) * self.height as f32 - 0.5).clamp(0.0, (self.height - 1) as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |x: usize, y: usize| self.rgb[y * self.width + x];
        at(x0, y0).lerp(at(x1, y0), fx).lerp(at(x0, y1).lerp(at(x1, y1), fx), fy)
    }
}

/// Diffuse and specular layers of a Blender MatCap EXR.
fn read_exr(bytes: &[u8]) -> Option<[self::Layer; 2]> {
    use exr::prelude::*;
    let image = read()
        .no_deep_data()
        .largest_resolution_level()
        .all_channels()
        .first_valid_layer()
        .all_attributes()
        .from_buffered(std::io::Cursor::new(bytes))
        .ok()?;
    let layer = &image.layer_data;
    let (width, height) = (layer.size.0, layer.size.1);
    let channel = |name: String| -> Option<Vec<f32>> {
        let ch = layer.channel_data.list.iter().find(|c| c.name.to_string() == name)?;
        Some(ch.sample_data.values_as_f32().collect())
    };
    let layer_of = |prefix: &str| -> Option<self::Layer> {
        let (r, g, b) = (channel(format!("{prefix}.R"))?, channel(format!("{prefix}.G"))?, channel(format!("{prefix}.B"))?);
        let rgb = (0..width * height).map(|i| Vec3::new(r[i], g[i], b[i]).max(Vec3::ZERO)).collect();
        Some(self::Layer { width, height, rgb })
    };
    Some([layer_of("diffuse")?, layer_of("specular")?])
}

fn srgb_u8_to_linear(b: u8) -> f32 {
    let c = b as f32 / 255.0;
    if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn linear_to_srgb_u8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

const KEY: Vec3 = Vec3::new(-0.45, 0.65, 0.62);

fn key() -> Vec3 {
    KEY.normalize()
}

fn lambert_wrap(n: Vec3, l: Vec3, wrap: f32) -> f32 {
    ((n.dot(l) + wrap) / (1.0 + wrap)).clamp(0.0, 1.0)
}

fn spec(n: Vec3, l: Vec3, power: f32) -> f32 {
    let h = (l + Vec3::Z).normalize();
    n.dot(h).max(0.0).powf(power)
}

fn fresnel(n: Vec3, power: f32) -> f32 {
    (1.0 - n.z).clamp(0.0, 1.0).powf(power)
}

fn clay(n: Vec3) -> Vec3 {
    let d = lambert_wrap(n, key(), 0.35);
    let bounce = (0.5 - n.y * 0.5) * 0.08;
    Vec3::splat(0.16 + 0.62 * d + bounce) + Vec3::splat(spec(n, key(), 12.0) * 0.06)
}

fn clay_dark(n: Vec3) -> Vec3 {
    clay(n) * 0.38
}

fn gloss(n: Vec3) -> Vec3 {
    let d = lambert_wrap(n, key(), 0.2);
    let base = Vec3::splat(0.08 + 0.55 * d);
    base + Vec3::splat(spec(n, key(), 90.0) * 0.9 + fresnel(n, 4.0) * 0.18)
}

fn chrome(n: Vec3) -> Vec3 {
    // Reflect the view vector and look up a studio "horizon" environment.
    let r = 2.0 * n.z * n - Vec3::Z;
    let y = r.y;
    let sky = Vec3::new(0.85, 0.88, 0.92);
    let ground = Vec3::new(0.05, 0.05, 0.055);
    let mut c = if y > 0.0 {
        sky.lerp(Vec3::splat(0.35), (y * 1.6).min(1.0))
    } else {
        ground.lerp(Vec3::splat(0.22), (-y * 2.0).min(1.0))
    };
    // Bright horizon band and a softbox.
    c += Vec3::splat((1.0 - (y.abs() * 25.0).min(1.0)) * 0.9);
    let softbox = r.dot(Vec3::new(-0.5, 0.6, 0.62).normalize()).max(0.0).powf(60.0);
    c + Vec3::splat(softbox * 1.5)
}

fn red_wax(n: Vec3) -> Vec3 {
    let d = lambert_wrap(n, key(), 0.6);
    let base = Vec3::new(0.55, 0.09, 0.06);
    let sss = Vec3::new(0.35, 0.05, 0.02) * fresnel(n, 1.5);
    base * (0.2 + 0.9 * d) + sss + Vec3::splat(spec(n, key(), 30.0) * 0.25)
}

fn jade(n: Vec3) -> Vec3 {
    let d = lambert_wrap(n, key(), 0.7);
    let base = Vec3::new(0.12, 0.38, 0.22);
    base * (0.3 + 0.8 * d)
        + Vec3::new(0.2, 0.45, 0.3) * fresnel(n, 2.5) * 0.5
        + Vec3::splat(spec(n, key(), 70.0) * 0.5)
}

fn skin(n: Vec3) -> Vec3 {
    let d = lambert_wrap(n, key(), 0.5);
    let base = Vec3::new(0.62, 0.38, 0.28);
    base * (0.22 + 0.85 * d)
        + Vec3::new(0.4, 0.1, 0.05) * fresnel(n, 2.0) * 0.4
        + Vec3::splat(spec(n, key(), 20.0) * 0.08)
}

fn toon(n: Vec3) -> Vec3 {
    let d = n.dot(key());
    let band = if d > 0.55 {
        0.85
    } else if d > 0.1 {
        0.45
    } else {
        0.18
    };
    let rim = if fresnel(n, 1.0) > 0.82 { 0.25 } else { 0.0 };
    Vec3::splat(band + rim)
}

fn rim_light(n: Vec3) -> Vec3 {
    let d = lambert_wrap(n, key(), 0.0);
    Vec3::splat(0.03 + 0.18 * d) + Vec3::new(0.8, 0.85, 1.0) * fresnel(n, 3.0) * 1.1
}

fn normals(n: Vec3) -> Vec3 {
    let c = n * 0.5 + Vec3::splat(0.5);
    // Stored as sRGB, so undo the encoding to keep the familiar normal-map colors.
    Vec3::new(c.x.powf(2.2), c.y.powf(2.2), c.z.powf(2.2))
}

