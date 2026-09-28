//! Built-in HDR environments for the Rendered mode: Blender's own world HDRIs (Forest, Studio,
//! Sunset; Poly Haven, CC0, see `assets/hdri/LICENSE.txt`), embedded in the executable so the
//! Rendered mode looks like Blender's Material Preview out of the box. Users can add their own.
//!
//! Equirectangular layout as in Blender, Z-up: u = 0.5 - atan2(y, x) / 2π, v = acos(z) / π
//! (0 = zenith).

use crate::loader::EnvImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Preset {
    /// Blender's Material Preview default.
    #[serde(alias = "Outdoor")]
    Forest,
    Studio,
    Sunset,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Forest, Preset::Studio, Preset::Sunset];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Forest => "Forest",
            Preset::Studio => "Studio",
            Preset::Sunset => "Sunset",
        }
    }

    fn exr(self) -> &'static [u8] {
        match self {
            Preset::Forest => include_bytes!("../../assets/hdri/forest.exr"),
            Preset::Studio => include_bytes!("../../assets/hdri/studio.exr"),
            Preset::Sunset => include_bytes!("../../assets/hdri/sunset.exr"),
        }
    }
}

/// Decodes a built-in environment (1024 x 512, a few milliseconds).
pub fn load(preset: Preset) -> EnvImage {
    let img = image::load_from_memory_with_format(preset.exr(), image::ImageFormat::OpenExr)
        .expect("built-in HDRIs are valid")
        .to_rgb32f();
    let (width, height) = img.dimensions();
    EnvImage { width, height, pixels: img.pixels().map(|p| [p[0], p[1], p[2], 1.0]).collect() }
}

/// Small preview strip for the UI (sRGB bytes, simple Reinhard so bright lights don't clip),
/// box-filtered down from `env`.
pub fn thumbnail(env: &EnvImage, width: u32, height: u32) -> Vec<u8> {
    let (sw, sh) = (env.width as usize, env.height as usize);
    let (w, h) = (width as usize, height as usize);
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let (y0, y1) = (y * sh / h, ((y + 1) * sh / h).max(y * sh / h + 1).min(sh));
        for x in 0..w {
            let (x0, x1) = (x * sw / w, ((x + 1) * sw / w).max(x * sw / w + 1).min(sw));
            let mut acc = [0f32; 3];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = env.pixels[sy * sw + sx];
                    for c in 0..3 {
                        acc[c] += p[c];
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)).max(1) as f32;
            for c in acc {
                let v = c / n;
                let v = v / (1.0 + v);
                let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
                out.push((s.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
            }
            out.push(255);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_environments_decode_quickly() {
        for preset in Preset::ALL {
            let start = std::time::Instant::now();
            let env = load(preset);
            let ms = start.elapsed().as_secs_f32() * 1000.0;
            println!("{preset:?}: {}x{} in {ms:.1} ms", env.width, env.height);
            assert_eq!((env.width, env.height), (1024, 512));
            assert!(env.pixels.iter().any(|p| p[0] > 10.0), "HDR range kept");
        }
    }
}
