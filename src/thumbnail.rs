//! `polyloupe --thumbnail IN OUT.png [SIZE]`: renders the Explorer thumbnail of a model with the
//! same software renderer the thumbnail handler uses, so it shows exactly what Explorer will.

use std::path::Path;

use crate::loader;

pub fn run(input: &Path, output: &Path, size: u32) -> Result<(), String> {
    let size = size.clamp(16, 1024);
    let scene = loader::load(input)?;
    let rgba = polyloupe_core::thumbnail::render(&scene, size).ok_or("Nothing to draw")?;
    image::save_buffer(output, &rgba, size, size, image::ColorType::Rgba8).map_err(|e| e.to_string())
}
