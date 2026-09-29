//! Color space helpers.

pub fn srgb_to_linear(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| srgb_channel_to_linear(v as f32 / 255.0))
}

pub fn srgb_channel_to_linear(s: f32) -> f32 {
    if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
}
