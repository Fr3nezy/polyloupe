//! Window and taskbar icon: the Poly Loupe app icon (see `assets/brand/make_brand.py`).

use eframe::egui::IconData;

pub fn app_icon() -> IconData {
    let png = include_bytes!("../assets/brand/icon/icon-256.png");
    let rgba = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .expect("bundled icon is a valid PNG")
        .to_rgba8();
    IconData { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() }
}
