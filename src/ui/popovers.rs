//! Viewport overlays popover and the shading options (drawn in the inspector's Shading tab).

use crate::i18n::tr;
use eframe::egui::{
    self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, Vec2, pos2,
};

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use super::{theme, widgets};
use crate::loader::{self, EnvImage};
use crate::render::{environment, matcap};
use crate::settings::{
    ColorMode, Environment, Lighting, Settings, ShadingMode, TexturePass, ViewTransform,
};

const ENV_THUMB: [u32; 2] = [128, 64];

/// Lazily created preview textures for the popovers.
pub struct Thumbnails {
    matcaps: Vec<TextureHandle>,
    /// Environment previews by path (or preset label); `None` while decoding or when unreadable.
    custom: HashMap<String, Option<TextureHandle>>,
    custom_tx: Sender<(String, Option<Vec<u8>>)>,
    custom_rx: Receiver<(String, Option<Vec<u8>>)>,
}

impl Default for Thumbnails {
    fn default() -> Self {
        let (custom_tx, custom_rx) = channel();
        Self { matcaps: Vec::new(), custom: HashMap::new(), custom_tx, custom_rx }
    }
}

impl Thumbnails {
    /// Stores the preview of an HDRI that was just loaded, so it is never decoded twice.
    pub fn add_environment(&mut self, ctx: &egui::Context, path: &str, env: &EnvImage) {
        let pixels = environment::thumbnail(env, ENV_THUMB[0], ENV_THUMB[1]);
        self.custom.insert(path.to_string(), Some(env_texture(ctx, path, &pixels)));
    }

    fn env_texture(&mut self, ctx: &egui::Context, env: &Environment) -> Option<egui::TextureId> {
        for (p, pixels) in self.custom_rx.try_iter() {
            let tex = pixels.map(|px| env_texture(ctx, &p, &px));
            self.custom.insert(p, tex);
        }
        let key = match env {
            Environment::Preset(p) => p.label().to_string(),
            Environment::File(path) => path.clone(),
        };
        if !self.custom.contains_key(&key) {
            // Decoding an HDRI takes from ~15 ms (built-in) to seconds (8K): off the UI thread.
            self.custom.insert(key.clone(), None);
            let (tx, env, ctx, k) = (self.custom_tx.clone(), env.clone(), ctx.clone(), key.clone());
            std::thread::spawn(move || {
                let decoded = match env {
                    Environment::Preset(p) => Ok(environment::load(p)),
                    Environment::File(path) => loader::load_environment(std::path::Path::new(&path)),
                };
                let pixels = decoded.ok().map(|e| environment::thumbnail(&e, ENV_THUMB[0], ENV_THUMB[1]));
                let _ = tx.send((k, pixels));
                ctx.request_repaint();
            });
        }
        self.custom.get(&key).and_then(|t| t.as_ref().map(|t| t.id()))
    }
}

fn env_texture(ctx: &egui::Context, name: &str, pixels: &[u8]) -> TextureHandle {
    let image = egui::ColorImage::from_rgba_unmultiplied([ENV_THUMB[0] as usize, ENV_THUMB[1] as usize], pixels);
    ctx.load_texture(format!("env_{name}"), image, TextureOptions::LINEAR)
}

pub enum PopoverAction {
    LoadHdri,
    SetDefault(Environment),
    RemoveEnvironment(String),
}

pub fn overlays(ui: &mut Ui, s: &mut Settings) {
    ui.set_min_width(220.0);
    widgets::section(ui, "Viewport Overlays");
    ui.add_enabled_ui(s.show_overlays, |ui| {
        ui.checkbox(&mut s.show_grid, tr("Floor grid"));
        ui.add_enabled_ui(s.show_grid, |ui| {
            ui.indent("axes", |ui| ui.checkbox(&mut s.show_axes, tr("Axes")));
        });
        ui.checkbox(&mut s.show_stats, tr("Statistics"));
        ui.checkbox(&mut s.show_gizmo, tr("Navigation gizmo"));
        ui.add_enabled_ui(s.shading != ShadingMode::Wireframe, |ui| {
            ui.checkbox(&mut s.show_wire_overlay, tr("Wireframe"));
        });
    });
    ui.separator();
    widgets::section(ui, "Performance");
    ui.checkbox(&mut s.vsync, tr("V-Sync"))
        .on_hover_text(tr("Off: frames aren't capped to the monitor refresh rate"));
    ui.checkbox(&mut s.show_fps, tr("Frame rate"))
        .on_hover_text(tr("Redraws continuously and shows FPS in the status bar"));
}

pub fn shading(ui: &mut Ui, s: &mut Settings, thumbs: &mut Thumbnails) -> Option<PopoverAction> {
    ui.set_min_width(270.0);
    let mut action = None;
    match s.shading {
        ShadingMode::Solid => solid(ui, s, thumbs),
        ShadingMode::Wireframe => {
            widgets::section(ui, "Wireframe Color");
            let mut random = s.color == ColorMode::Random;
            ui.horizontal(|ui| {
                if widgets::segmented(ui, &mut random, &[(false, "Theme"), (true, "Random")]) {
                    s.color = if random { ColorMode::Random } else { ColorMode::Material };
                }
            });
        }
        ShadingMode::Rendered => action = rendered(ui, s, thumbs),
    }
    ui.add_space(4.0);
    ui.separator();
    widgets::section(ui, "Options");
    let mut xray = s.xray();
    if ui.checkbox(&mut xray, tr("X-Ray")).changed() {
        s.toggle_xray();
    }
    ui.add_enabled_ui(xray, |ui| {
        ui.add(egui::Slider::new(&mut s.xray_alpha, 0.05..=1.0).text(tr("Alpha")));
    });
    if s.shading != ShadingMode::Wireframe {
        ui.checkbox(&mut s.backface_culling, tr("Backface culling"));
    }
    if s.shading == ShadingMode::Solid {
        ui.checkbox(&mut s.show_outline, tr("Outline"));
    }
    action
}

fn solid(ui: &mut Ui, s: &mut Settings, thumbs: &mut Thumbnails) {
    widgets::section(ui, "Lighting");
    ui.horizontal(|ui| {
        widgets::segmented(
            ui,
            &mut s.lighting,
            &[(Lighting::Studio, "Studio"), (Lighting::MatCap, "MatCap"), (Lighting::Flat, "Flat")],
        )
    });
    if s.lighting == Lighting::MatCap {
        ui.add_space(4.0);
        matcap_picker(ui, s, thumbs);
    }
    ui.add_space(4.0);
    widgets::section(ui, "Color");
    ui.horizontal(|ui| {
        widgets::segmented(
            ui,
            &mut s.color,
            &[(ColorMode::Material, "Material"), (ColorMode::Single, "Single"), (ColorMode::Random, "Random")],
        )
    });
    ui.horizontal(|ui| {
        widgets::segmented(
            ui,
            &mut s.color,
            &[(ColorMode::Texture, "Texture"), (ColorMode::Attribute, "Attribute")],
        )
    })
    .response
    .on_hover_text(tr("Texture: image maps and their passes · Attribute: vertex colors"));
    match s.color {
        ColorMode::Single => {
            ui.horizontal(|ui| {
                ui.color_edit_button_srgb(&mut s.single_color);
                ui.label(RichText::new(tr("Object color")).color(theme::TEXT_DIM));
            });
        }
        ColorMode::Texture => {
            ui.add_space(2.0);
            widgets::section(ui, "Pass");
            for row in TexturePass::ALL.chunks(4) {
                ui.horizontal(|ui| {
                    let options: Vec<(TexturePass, &str)> = row.iter().map(|p| (*p, p.label())).collect();
                    widgets::segmented(ui, &mut s.texture_pass, &options);
                });
            }
            let hint = match s.texture_pass {
                TexturePass::BaseColor | TexturePass::UvChecker => tr("Lit with the current lighting"),
                _ => tr("Raw values, unlit (like Blender's Non-Color)"),
            };
            ui.label(RichText::new(hint).size(11.0).color(theme::TEXT_DIM));
        }
        _ => {}
    }
}

fn rendered(ui: &mut Ui, s: &mut Settings, thumbs: &mut Thumbnails) -> Option<PopoverAction> {
    let mut action = None;
    widgets::section(ui, "Environment");
    let mut items: Vec<Environment> = environment::Preset::ALL.into_iter().map(Environment::Preset).collect();
    items.extend(s.custom_environments.iter().cloned().map(Environment::File));
    for row in items.chunks(3) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for env in row {
                let texture = thumbs.env_texture(ui.ctx(), env);
                let name = match env {
                    Environment::Preset(p) => tr(p.label()).to_string(),
                    Environment::File(path) => file_label(path),
                };
                let is_default = *env == s.default_environment;
                let r = env_button(ui, texture, *env == s.environment, is_default);
                let hover = match env {
                    Environment::File(path) => format!("{name}\n{path}"),
                    Environment::Preset(_) => name,
                };
                let r = r.on_hover_text(format!("{hover}\n{}", tr("Right-click for options")));
                if r.clicked() {
                    s.environment = env.clone();
                }
                r.context_menu(|ui| {
                    if ui.add_enabled(!is_default, egui::Button::new(tr("Use as default"))).clicked() {
                        action = Some(PopoverAction::SetDefault(env.clone()));
                        ui.close();
                    }
                    if let Environment::File(path) = env {
                        if ui.button(tr("Remove from list")).clicked() {
                            action = Some(PopoverAction::RemoveEnvironment(path.clone()));
                            ui.close();
                        }
                    }
                });
            }
        });
    }
    ui.horizontal(|ui| {
        if ui.button(tr("Load HDRI…")).clicked() {
            action = Some(PopoverAction::LoadHdri);
        }
        ui.label(RichText::new(tr("or drop a .hdr / .exr")).size(11.0).color(theme::TEXT_DIM));
    });
    ui.add(egui::Slider::new(&mut s.env_rotation, -180.0..=180.0).suffix("°").text(tr("Rotation")));
    ui.add(egui::Slider::new(&mut s.env_strength, 0.0..=4.0).text(tr("Strength")));
    ui.checkbox(&mut s.env_background, tr("World background"));
    ui.add_enabled_ui(s.env_background, |ui| {
        ui.add(egui::Slider::new(&mut s.env_blur, 0.0..=1.0).text(tr("Blur")));
    });
    ui.add_space(4.0);
    widgets::section(ui, "Color Management");
    ui.horizontal(|ui| {
        widgets::segmented(
            ui,
            &mut s.view_transform,
            &[(ViewTransform::AgX, "AgX"), (ViewTransform::Standard, "Standard")],
        )
    });
    ui.add(egui::Slider::new(&mut s.exposure, 0.1..=4.0).logarithmic(true).text(tr("Exposure")));
    action
}

fn file_label(path: &str) -> String {
    std::path::Path::new(path).file_stem().map_or(path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Environment preview; the default one carries an accent dot in its corner.
fn env_button(ui: &mut Ui, texture: Option<egui::TextureId>, selected: bool, is_default: bool) -> egui::Response {
    let size = Vec2::new(84.0, 42.0);
    let response = match texture {
        Some(t) => thumb_button(ui, t, size, selected),
        None => {
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            ui.painter().rect_filled(rect, CornerRadius::same(4), theme::BG_APP);
            let spinner = Rect::from_center_size(rect.center(), Vec2::splat(14.0));
            ui.put(spinner, egui::Spinner::new().size(14.0).color(theme::TEXT_DIM));
            let stroke = if selected { Stroke::new(2.0, theme::ACCENT) } else { Stroke::new(1.0, theme::BORDER) };
            ui.painter().rect_stroke(rect, CornerRadius::same(4), stroke, StrokeKind::Inside);
            response
        }
    };
    if is_default {
        let c = response.rect.left_top() + Vec2::new(8.0, 8.0);
        ui.painter().circle(c, 4.0, theme::ACCENT, Stroke::new(1.5, theme::BG_APP));
    }
    response
}

fn thumb_button(ui: &mut Ui, texture: egui::TextureId, size: Vec2, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(4), theme::BG_APP);
    painter.image(texture, rect.shrink(3.0), Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
    let stroke = if selected {
        Stroke::new(2.0, theme::ACCENT)
    } else if response.hovered() {
        Stroke::new(1.0, theme::TEXT_DIM)
    } else {
        Stroke::new(1.0, theme::BORDER)
    };
    painter.rect_stroke(rect, CornerRadius::same(4), stroke, StrokeKind::Inside);
    response
}

fn matcap_picker(ui: &mut Ui, s: &mut Settings, thumbs: &mut Thumbnails) {
    if thumbs.matcaps.is_empty() {
        for (i, preset) in matcap::PRESETS.iter().enumerate() {
            let pixels = matcap::generate(i);
            let image = egui::ColorImage::from_rgba_unmultiplied([matcap::SIZE, matcap::SIZE], &pixels);
            thumbs
                .matcaps
                .push(ui.ctx().load_texture(format!("matcap_{}", preset.name), image, TextureOptions::LINEAR));
        }
    }
    const PER_ROW: usize = 5;
    for (row, chunk) in thumbs.matcaps.chunks(PER_ROW).enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (k, thumb) in chunk.iter().enumerate() {
                let i = row * PER_ROW + k;
                let r = thumb_button(ui, thumb.id(), Vec2::splat(46.0), s.matcap == i)
                    .on_hover_text(matcap::PRESETS[i].name);
                if r.clicked() {
                    s.matcap = i;
                }
            }
        });
    }
}

pub enum ChannelAction {
    Show(TexturePass),
    /// Back to the normal color mode (for the selection, or the whole scene).
    Off,
    ClearOverrides,
}
