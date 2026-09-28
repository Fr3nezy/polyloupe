//! Procedurally generated MatCaps: no bundled images, no licensing questions.
//!
//! Each preset shades a hemisphere of view-space normals; the result is stored as sRGB bytes
//! so the same pixels feed both the GPU texture and the egui thumbnails.

use glam::Vec3;

pub const SIZE: usize = 256;

#[derive(Clone, Copy)]
pub struct Preset {
    pub name: &'static str,
    shade: fn(n: Vec3) -> Vec3,
}

pub const PRESETS: &[Preset] = &[
    Preset { name: "Clay", shade: clay },
    Preset { name: "Clay Dark", shade: clay_dark },
    Preset { name: "Studio Gloss", shade: gloss },
    Preset { name: "Chrome", shade: chrome },
    Preset { name: "Red Wax", shade: red_wax },
    Preset { name: "Jade", shade: jade },
    Preset { name: "Skin", shade: skin },
    Preset { name: "Toon", shade: toon },
    Preset { name: "Rim Light", shade: rim_light },
    Preset { name: "Normals", shade: normals },
];

/// Returns SIZE*SIZE sRGB RGBA8 pixels.
pub fn generate(index: usize) -> Vec<u8> {
    let preset = PRESETS[index.min(PRESETS.len() - 1)];
    let mut out = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let u = (x as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
            let v = -((y as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0);
            // Clamp outside the disc to its rim so edge samples never pick up garbage.
            let r2 = u * u + v * v;
            let (u, v) = if r2 > 0.999 {
                let s = 0.999f32.sqrt() / r2.sqrt();
                (u * s, v * s)
            } else {
                (u, v)
            };
            let n = Vec3::new(u, v, (1.0 - u * u - v * v).max(0.0).sqrt());
            let c = (preset.shade)(n);
            for ch in c.to_array() {
                out.push(linear_to_srgb_u8(ch));
            }
            out.push(255);
        }
    }
    out
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
