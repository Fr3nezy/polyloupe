//! Poly Loupe design tokens: black chrome, one white accent, hairline borders, 4px corners.
//! Space Grotesk for text, a monospace for technical labels (uppercase, tracked out).

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow,
    Stroke, TextStyle, Vec2,
    text::{LayoutJob, TextFormat},
};

/// Window chrome, panels.
pub const BG_APP: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
pub const PANEL: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
/// Toolbars, cards, popovers, idle controls.
pub const SURFACE: Color32 = Color32::from_rgb(0x11, 0x11, 0x11);
pub const WIDGET: Color32 = SURFACE;
pub const WIDGET_HOVER: Color32 = Color32::from_rgb(0x1c, 0x1c, 0x1c);
pub const WIDGET_PRESS: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
/// Hairlines between regions and around controls.
pub const BORDER: Color32 = Color32::from_rgb(0x2b, 0x2b, 0x2b);
/// Emphasized outline: warnings, dashed drop zones.
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x6b, 0x6b, 0x6b);
pub const TEXT: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0xa3, 0xa3, 0xa3);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x5c, 0x5c, 0x5c);
/// The one accent is white: selected controls invert to white on black.
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
pub const ACCENT_HOVER: Color32 = Color32::from_rgb(0xd9, 0xd9, 0xd9);
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
pub const ERROR: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);
/// Behind the 3D view (matches the renderer's clear color).
pub const VIEWPORT: Color32 = Color32::from_rgb(0x26, 0x26, 0x26);
/// Selection in the 3D view and outliner stays Blender orange: it is information, not chrome.
pub const SELECTION: Color32 = Color32::from_rgb(0xe0, 0x89, 0x2a);
/// Mesh analysis markers; keep in sync with `marker_color` in mesh.wgsl.
pub const MARK_NON_MANIFOLD: Color32 = Color32::from_rgb(0xff, 0x2e, 0x78);
pub const MARK_OPEN: Color32 = Color32::from_rgb(0xff, 0xc2, 0x33);
pub const MARK_OVERLAP: Color32 = Color32::from_rgb(0x33, 0xdb, 0xff);
/// Normal lines; keep in sync with `fs_normal` in mesh.wgsl.
pub const MARK_NORMAL: Color32 = Color32::from_rgb(0x59, 0xb3, 0xff);
/// Section plane outline and cap (the cap hatching in mesh.wgsl uses the same red).
pub const SECTION: Color32 = Color32::from_rgb(0xd9, 0x4d, 0x38);

pub const AXIS_X: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);
pub const AXIS_Y: Color32 = Color32::from_rgb(0x46, 0xa7, 0x58);
pub const AXIS_Z: Color32 = Color32::from_rgb(0x3e, 0x8e, 0xd0);

pub const RADIUS: u8 = 4;
pub const CONTROL_HEIGHT: f32 = 28.0;
/// Toolbar buttons (viewport toolbar, tool rail is 40).
pub const TOOLBAR_HEIGHT: f32 = 32.0;

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// Technical label: monospace, uppercase, tracked out (section titles, HUD, footer, tags).
pub fn caps(text: &str, size: f32, color: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.append(
        &text.to_uppercase(),
        0.0,
        TextFormat { font_id: mono(size), color, extra_letter_spacing: size * 0.14, ..Default::default() },
    );
    job
}

pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, bold(18.0)),
            (TextStyle::Monospace, mono(12.0)),
        ]
        .into();

        let s = &mut style.spacing;
        s.item_spacing = Vec2::new(6.0, 6.0);
        s.button_padding = Vec2::new(10.0, 4.0);
        s.interact_size = Vec2::new(28.0, CONTROL_HEIGHT);
        s.menu_margin = Margin::same(4);
        s.window_margin = Margin::same(12);
        s.slider_width = 150.0;
        s.combo_height = 260.0;

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.panel_fill = BG_APP;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_corner_radius = CornerRadius::same(RADIUS);
        v.menu_corner_radius = CornerRadius::same(RADIUS);
        v.window_shadow = Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(160) };
        v.popup_shadow = v.window_shadow;
        v.extreme_bg_color = Color32::from_rgb(0x0a, 0x0a, 0x0a);
        v.faint_bg_color = SURFACE;
        v.code_bg_color = SURFACE;
        v.override_text_color = None;
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.0, ON_ACCENT);
        v.hyperlink_color = TEXT;
        v.error_fg_color = ERROR;
        v.warn_fg_color = TEXT;
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };

        let radius = CornerRadius::same(RADIUS);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = SURFACE;
        w.noninteractive.weak_bg_fill = SURFACE;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
        w.noninteractive.corner_radius = radius;

        w.inactive.bg_fill = SURFACE;
        w.inactive.weak_bg_fill = SURFACE;
        w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = radius;

        w.hovered.bg_fill = WIDGET_HOVER;
        w.hovered.weak_bg_fill = WIDGET_HOVER;
        w.hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
        w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.corner_radius = radius;
        w.hovered.expansion = 0.0;

        w.active.bg_fill = WIDGET_PRESS;
        w.active.weak_bg_fill = WIDGET_PRESS;
        w.active.bg_stroke = Stroke::new(1.0, TEXT);
        w.active.fg_stroke = Stroke::new(1.0, TEXT);
        w.active.corner_radius = radius;
        w.active.expansion = 0.0;

        w.open.bg_fill = WIDGET_PRESS;
        w.open.weak_bg_fill = WIDGET_PRESS;
        w.open.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
        w.open.fg_stroke = Stroke::new(1.0, TEXT);
        w.open.corner_radius = radius;
    });
}

/// Space Grotesk (bundled, OFL) for text; Consolas for technical labels on Windows. egui's
/// bundled fonts and Segoe UI Symbol stay as fallbacks for glyph coverage (arrows, ✕...).
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let bundled: [(&str, &'static [u8]); 3] = [
        ("space_regular", include_bytes!("../../assets/fonts/SpaceGrotesk-Regular.ttf")),
        ("space_medium", include_bytes!("../../assets/fonts/SpaceGrotesk-Medium.ttf")),
        ("space_bold", include_bytes!("../../assets/fonts/SpaceGrotesk-Bold.ttf")),
    ];
    for (name, bytes) in bundled {
        fonts.font_data.insert(name.into(), FontData::from_static(bytes).into());
    }
    let mut system = Vec::new();
    for (name, path) in [("consolas", r"C:\Windows\Fonts\consola.ttf"), ("segoe_symbol", r"C:\Windows\Fonts\seguisym.ttf")] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(name.into(), FontData::from_owned(bytes).into());
            system.push(name);
        }
    }
    let fallbacks: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let symbol = system.contains(&"segoe_symbol").then(|| "segoe_symbol".to_string());
    let family = |primary: &str| {
        let mut f = vec![primary.to_string()];
        f.extend(fallbacks.iter().cloned());
        f.extend(symbol.clone());
        f
    };
    fonts.families.insert(FontFamily::Proportional, family("space_regular"));
    fonts.families.insert(FontFamily::Name("medium".into()), family("space_medium"));
    fonts.families.insert(FontFamily::Name("bold".into()), family("space_bold"));
    if system.contains(&"consolas") {
        if let Some(mono) = fonts.families.get_mut(&FontFamily::Monospace) {
            mono.insert(0, "consolas".into());
        }
    }
    ctx.set_fonts(fonts);
}
