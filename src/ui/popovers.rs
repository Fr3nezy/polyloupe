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
    ColorMode, Environment, Lighting, MaterialGroup, MetalFinish, PartMaterial, Settings, ShadingMode, TexturePass,
    ViewTransform,
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
        ui.checkbox(&mut s.show_bounds_overlay, tr("Bounding box"))
            .on_hover_text(tr("Show model dimensions and bounding box axes"));
        ui.add_enabled_ui(s.shading != ShadingMode::Wireframe, |ui| {
            ui.checkbox(&mut s.show_wire_overlay, tr("Wireframe"));
            if s.show_wire_overlay {
                ui.indent("wire_opts", |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(tr("Color")).size(11.0).color(theme::TEXT_DIM));
                        widgets::segmented(ui, &mut s.wire_color_mode, &[
                            (crate::settings::WireColorMode::Theme, "Theme"),
                            (crate::settings::WireColorMode::Random, "Random"),
                            (crate::settings::WireColorMode::Custom, "Custom"),
                        ]);
                    });
                    if s.wire_color_mode == crate::settings::WireColorMode::Custom {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(tr("Color picker")).size(11.0).color(theme::TEXT_DIM));
                            ui.color_edit_button_srgb(&mut s.wire_color);
                        });
                    }
                    ui.add(egui::Slider::new(&mut s.wire_opacity, 0.1..=1.0).text(tr("Opacity")));
                });
            }
        });
        ui.checkbox(&mut s.show_origins, tr("Origins"))
            .on_hover_text(tr("Each object's pivot as a dot, like Blender"));
        ui.add_enabled_ui(s.shading == ShadingMode::Solid, |ui| {
            ui.checkbox(&mut s.show_outline, tr("Object outlines"))
                .on_hover_text(tr("Dark line around each object in Solid mode"));
        });
    });
    ui.separator();
    widgets::section(ui, "Mesh Analysis");
    ui.add_enabled_ui(s.show_overlays, |ui| {
        marker_toggle(ui, &mut s.show_non_manifold, "Non-manifold edges", theme::MARK_NON_MANIFOLD)
            .on_hover_text(tr("Edges shared by more than two faces, or between faces with flipped normals"));
        marker_toggle(ui, &mut s.show_open_edges, "Open edges", theme::MARK_OPEN)
            .on_hover_text(tr("Edges with a single face: holes and open borders"));
        marker_toggle(ui, &mut s.show_overlapping, "Overlapping vertices", theme::MARK_OVERLAP)
            .on_hover_text(tr("Separate vertices closer than 0.1 mm, what Merge by Distance would weld"));
        marker_toggle(ui, &mut s.show_normals, "Normals", theme::MARK_NORMAL)
            .on_hover_text(tr("A line along each vertex normal"));
        if s.show_normals {
            ui.indent("normal_size", |ui| {
                ui.add(egui::Slider::new(&mut s.normal_size, 0.002..=0.2).logarithmic(true).show_value(false).text(tr("Length")));
            });
        }
        ui.checkbox(&mut s.show_face_orientation, tr("Face orientation"))
            .on_hover_text(tr("Front faces blue, back faces red: flipped faces show up red"));
    });
    ui.separator();
    widgets::section(ui, "Performance");
    ui.checkbox(&mut s.vsync, tr("V-Sync"))
        .on_hover_text(tr("Off: frames aren't capped to the monitor refresh rate"));
    ui.checkbox(&mut s.show_fps, tr("Frame rate"))
        .on_hover_text(tr("Redraws continuously and shows FPS in the status bar"));
}

/// `manufacturing`: the Manufacturing workspace, which adds the print finish to Rendered.
pub fn shading(ui: &mut Ui, s: &mut Settings, thumbs: &mut Thumbnails, manufacturing: bool) -> Option<PopoverAction> {
    ui.set_min_width(270.0);
    let mut action = None;
    match s.shading {
        ShadingMode::Solid => solid(ui, s, thumbs, manufacturing),
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
    action
}

/// What a part is made of on screen (Manufacturing): one color and a material, or the file's own
/// materials; then wear (grain, scratches) over either.
pub fn part_material(ui: &mut Ui, s: &mut Settings) {
    widgets::section(ui, "Part Material");
    ui.horizontal(|ui| {
        widgets::segmented(ui, &mut s.file_materials, &[(false, "One color"), (true, "File materials")]);
    });
    if s.file_materials {
        ui.label(RichText::new(tr("The colors and materials stored in the file")).size(11.0).color(theme::TEXT_DIM));
    } else {
        ui.horizontal(|ui| {
            ui.color_edit_button_srgb(&mut s.plastic_color);
            hex_field(ui, &mut s.plastic_color);
        });
        ui.add_space(4.0);
        let mut group = s.material.group();
        ui.horizontal(|ui| {
            let options: Vec<(MaterialGroup, &str)> = MaterialGroup::ALL.iter().map(|g| (*g, g.label())).collect();
            if widgets::segmented(ui, &mut group, &options) {
                s.material = group.first();
                if s.material.layer_height() > 0.0 {
                    s.layer_height = s.material.layer_height();
                }
            }
        });
        match group {
            MaterialGroup::Fdm => {
                ui.horizontal(|ui| {
                    let options: Vec<(PartMaterial, &str)> = PartMaterial::FDM.iter().map(|m| (*m, m.label())).collect();
                    widgets::segmented(ui, &mut s.material, &options);
                });
            }
            MaterialGroup::Metal => {
                ui.horizontal(|ui| {
                    let options: Vec<(MetalFinish, &str)> = MetalFinish::ALL.iter().map(|f| (*f, f.label())).collect();
                    widgets::segmented(ui, &mut s.metal_finish, &options);
                });
            }
            _ => {}
        }
        ui.label(RichText::new(tr(s.material.description())).size(11.0).color(theme::TEXT_DIM));
        if s.material.layer_height() > 0.0 {
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.layer_lines, tr("Layer lines"));
                ui.add_enabled(s.layer_lines, egui::Slider::new(&mut s.layer_height, 0.02..=0.6).step_by(0.01).suffix(" mm"))
                    .on_hover_text(tr("Layer lines run along Z: lay the part on a face to change the print direction"));
            });
        }
        if s.shading != ShadingMode::Rendered {
            ui.label(RichText::new(tr("Rendered mode shows the material; Solid only its color and relief.")).size(11.0).color(theme::TEXT_FAINT));
        }
    }
    widgets::section(ui, "Surface Wear");
    let percent = |v: &mut f32, label: &str, tip: &str, ui: &mut Ui| {
        ui.add(egui::Slider::new(v, 0.0..=1.0)
            .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
            .custom_parser(|t| t.trim_end_matches('%').trim().parse::<f64>().ok().map(|v| v / 100.0))
            .text(tr(label)))
            .on_hover_text(tr(tip));
    };
    percent(&mut s.grain, "Grain", "Fine relief, uneven gloss and dust, so the part looks less like a perfect CG surface", ui);
    percent(&mut s.scratches, "Scratches", "Handling marks: on metal they catch the light, on plastic they whiten", ui);
    ui.add(egui::Slider::new(&mut s.grain_size, 0.2..=5.0).logarithmic(true).custom_formatter(|v, _| format!("×{v:.1}")).text(tr("Pattern size")))
        .on_hover_text(tr("Scale of the grain and scratch patterns"));
}

/// The color as #RRGGBB, editable.
fn hex_field(ui: &mut Ui, rgb: &mut [u8; 3]) {
    let id = ui.id().with("hex");
    let current = format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
    let mut text = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| current.clone());
    let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(78.0).font(theme::mono(12.0)));
    if r.changed() {
        let hex = text.trim().trim_start_matches('#');
        if hex.len() == 6 {
            if let Ok(v) = u32::from_str_radix(hex, 16) {
                *rgb = [(v >> 16) as u8, (v >> 8) as u8, v as u8];
            }
        }
    }
    // While typing, keep what was typed; otherwise follow the color picker.
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    r.on_hover_text(tr("Hex color, like #D9D9D6"));
}

/// Checkbox with the marker color as a swatch after the label.
pub fn marker_toggle(ui: &mut Ui, value: &mut bool, label: &str, color: Color32) -> egui::Response {
    ui.horizontal(|ui| {
        let r = ui.checkbox(value, tr(label));
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
        ui.painter().rect_filled(rect, CornerRadius::same(2), color);
        r
    })
    .inner
}

fn solid(ui: &mut Ui, s: &mut Settings, thumbs: &mut Thumbnails, manufacturing: bool) {
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
    // Image maps and vertex colors belong to 3D Art; Manufacturing has neither.
    if !manufacturing {
        ui.horizontal(|ui| {
            widgets::segmented(
                ui,
                &mut s.color,
                &[(ColorMode::Texture, "Texture"), (ColorMode::Attribute, "Attribute")],
            )
        })
        .response
        .on_hover_text(tr("Texture: image maps and their passes · Attribute: vertex colors"));
    }
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
    widgets::section(ui, "Background");
    // Four choices over settings: Viewport, Studio, World, Transparent.
    let mut backdrop = if s.transparent_background {
        3
    } else if s.env_background {
        2
    } else if s.studio_backdrop {
        1
    } else {
        0
    };
    ui.horizontal(|ui| {
        if widgets::segmented(
            ui,
            &mut backdrop,
            &[(0, "Viewport"), (1, "Studio"), (2, "World"), (3, "Transparent")],
        ) {
            s.transparent_background = backdrop == 3;
            s.studio_backdrop = backdrop == 1;
            s.env_background = backdrop == 2;
            if s.transparent_background {
                s.export_transparent = true;
            }
        }
    });
    if s.transparent_background {
        ui.label(RichText::new(tr("Transparent background with checkerboard preview and alpha export")).size(11.0).color(theme::TEXT_DIM));
    } else if s.env_background {
        ui.add(egui::Slider::new(&mut s.env_blur, 0.0..=1.0).text(tr("Blur")));
    } else if s.studio_backdrop {
        ui.label(RichText::new(tr("Light gray sweep without the grid, for product shots")).size(11.0).color(theme::TEXT_DIM));
    }

    ui.label(RichText::new(tr("Lights & Shadows configured in Render tab")).size(11.0).color(theme::TEXT_DIM));

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
            let pixels = matcap::preview(i);
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
