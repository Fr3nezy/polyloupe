//! Application shell: window layout, input handling, file loading and viewport overlays.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Key, Layout, Margin, Modifiers,
    PointerButton, Pos2, Rect, RectAlign, RichText, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, UiBuilder, pos2, vec2,
};
use glam::Vec3;
use eframe::egui_wgpu;

mod shell;

use crate::anim::AnimPlayer;
use crate::camera::{AxisView, Camera};
use crate::cli::{CaptureOptions, LaunchOptions};
use crate::i18n::{self, Language, tr, trf};
use crate::instance;
use shell::{InspectorTab, ToastAction, Tool};
use crate::navigation::{DragInput, Gesture, Navigation};
use crate::loader::{self, EnvImage};
use crate::render::{self, FrameInput, Renderer, environment};
use crate::qa;
use crate::scene::{Aabb, MapRef, Scene};
use crate::settings::{ColorMode, Environment, Settings, ShadingMode, TexturePass};
use crate::ui::gizmo::{self, GizmoAction};
use crate::ui::pie::{PieChoice, PieMenu};
use crate::ui::popovers::{self, ChannelAction, PopoverAction, Thumbnails};
use crate::ui::sidebar::SidebarAction;
use crate::ui::timeline::{self, TimelineAction, TimelineView};
use crate::ui::widgets::{self};
use crate::ui::{icons, theme};

const APP_NAME: &str = "Poly Loupe";
/// Picks which decade of grid lines is visible for a given camera distance.
const GRID_LEVEL_OFFSET: f32 = 1.05;

struct ObjectMeta {
    name: String,
    triangles: usize,
    vertices: usize,
    /// World-space bounds.
    bounds: Aabb,
    material: usize,
}

struct MaterialMeta {
    name: String,
    maps: Vec<MapRef>,
}

struct SceneInfo {
    path: PathBuf,
    file_name: String,
    /// Texture files the model refers to but that weren't found.
    missing: Vec<String>,
    objects: Vec<ObjectMeta>,
    materials: Vec<MaterialMeta>,
    /// (name, width, height) per image.
    images: Vec<(String, u32, u32)>,
    vertices: usize,
    triangles: usize,
    load_time: Duration,
    /// Mesh analysis, filled in by a background thread shortly after loading.
    qa: Option<qa::Report>,
}

/// What a GPU pick is for.
#[derive(Clone, Copy)]
enum PickPurpose {
    Select { extend: bool },
    /// Place a measurement point.
    MeasureClick,
    /// Rubber band to the cursor while the second point is pending.
    MeasureHover,
}

/// Blender-style selection: a set plus one active object.
#[derive(Default)]
struct Selection {
    selected: Vec<bool>,
    active: Option<usize>,
}

impl Selection {
    fn reset(&mut self, count: usize) {
        self.selected = vec![false; count];
        self.active = None;
    }

    /// Viewport click: plain replaces the selection, Shift extends/toggles like Blender.
    fn click(&mut self, hit: Option<usize>, extend: bool) {
        match (hit, extend) {
            (Some(i), false) => {
                self.selected.fill(false);
                self.selected[i] = true;
                self.active = Some(i);
            }
            (Some(i), true) => {
                if self.selected[i] && self.active == Some(i) {
                    self.selected[i] = false;
                    self.active = None;
                } else {
                    self.selected[i] = true;
                    self.active = Some(i);
                }
            }
            (None, false) => self.clear(),
            (None, true) => {}
        }
    }

    fn clear(&mut self) {
        self.selected.fill(false);
        self.active = None;
    }

    fn all(&mut self, visible: &[bool]) {
        for (s, v) in self.selected.iter_mut().zip(visible) {
            *s = *v;
        }
    }

    fn any(&self) -> bool {
        self.selected.iter().any(|s| *s)
    }

    fn count(&self) -> usize {
        self.selected.iter().filter(|s| **s).count()
    }

    fn states(&self) -> Vec<u8> {
        self.selected
            .iter()
            .enumerate()
            .map(|(i, &s)| if self.active == Some(i) && s { 2 } else { s as u8 })
            .collect()
    }
}

struct Loading {
    path: PathBuf,
    rx: Receiver<Result<Scene, String>>,
    started: Instant,
}

struct Toast {
    text: String,
    until: f64,
    error: bool,
    /// A banner with an action stays until it's used or dismissed.
    action: Option<ToastAction>,
}

pub struct ViewerApp {
    settings: Settings,
    camera: Camera,
    renderer: Option<Renderer>,
    info: Option<SceneInfo>,
    loading: Option<Loading>,
    /// Mesh analysis of the loaded scene: totals plus marker instances per mesh.
    qa_rx: Option<Receiver<(qa::Report, Vec<Vec<render::MarkerInstance>>)>>,
    selection: Selection,
    visible: Vec<bool>,
    /// Per-object texture channel shown instead of the global color mode.
    pass_override: Vec<Option<TexturePass>>,
    /// Map previews for the sidebar, keyed by (image, channel).
    map_thumbs: HashMap<(usize, Option<u8>), TextureHandle>,
    anim: Option<AnimPlayer>,
    /// Set when the pose must be re-evaluated even though time didn't advance.
    anim_dirty: bool,
    /// Keyframe positions (frames) of the current clip, for the timeline.
    anim_keys: Vec<i32>,
    pie: PieMenu,
    toast: Option<Toast>,
    applied_vsync: Option<bool>,
    fps: f32,
    thumbs: Thumbnails,
    pending_open: Option<PathBuf>,
    pending_pick: Option<([u32; 2], PickPurpose)>,
    /// Finished measurements (world points) and the one being placed.
    measures: Vec<[Vec3; 2]>,
    measure_start: Option<Vec3>,
    /// Surface point under the cursor while a measurement is being placed.
    measure_hover: Option<Vec3>,
    measure_hover_px: Option<[u32; 2]>,
    env_loaded: Option<Environment>,
    env_loading: Option<(Environment, Receiver<Result<EnvImage, String>>)>,
    capture: Option<CaptureState>,
    show_prefs: bool,
    applied_language: Option<Language>,
    /// Receives files from later launches while "Open files in the same window" is on.
    instance: Option<instance::Server>,
    /// Last viewport size in pixels, for image export.
    viewport_px: [u32; 2],
    viewport_rect: Rect,
    tool: Tool,
    inspector_tab: InspectorTab,
    logo: Option<TextureHandle>,
    /// Extra folders to look for textures in (Materials > Choose texture folder), per model.
    texture_dirs: Vec<PathBuf>,
    /// Unity/Unreal fly mode (RMB held): WASD move the camera instead of running shortcuts.
    flying: bool,
    /// ZBrush: Shift was held while rotating, snap to an axis view on release.
    snap_on_release: bool,
}

/// Drives `--capture`: waits for the scene and view to settle, screenshots, saves, quits.
struct CaptureState {
    opts: CaptureOptions,
    frames: u32,
    requested: bool,
}

impl ViewerApp {
    pub fn new(cc: &eframe::CreationContext<'_>, launch: LaunchOptions) -> Self {
        theme::apply(&cc.egui_ctx);
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();
        // Every session starts from the default environment; picking another is a per-session look.
        settings.environment = settings.default_environment.clone();
        if let Some(c) = &launch.capture {
            c.apply(&mut settings);
        }
        let renderer = cc.wgpu_render_state.as_ref().map(|rs| {
            let info = rs.adapter.get_info();
            log::info!("GPU: {} ({:?}, {:?})", info.name, info.backend, info.device_type);
            Renderer::new(&rs.device, &rs.queue)
        });
        Self {
            settings,
            camera: Camera::default(),
            renderer,
            info: None,
            loading: None,
            qa_rx: None,
            selection: Selection::default(),
            visible: Vec::new(),
            pass_override: Vec::new(),
            map_thumbs: HashMap::new(),
            anim: None,
            anim_dirty: false,
            anim_keys: Vec::new(),
            pie: PieMenu::new(),
            toast: None,
            applied_vsync: None,
            fps: 0.0,
            thumbs: Thumbnails::default(),
            pending_open: launch.open,
            pending_pick: None,
            measures: Vec::new(),
            measure_start: None,
            measure_hover: None,
            measure_hover_px: None,
            env_loaded: None,
            env_loading: None,
            capture: launch.capture.map(|opts| CaptureState { opts, frames: 0, requested: false }),
            show_prefs: false,
            applied_language: None,
            instance: None,
            viewport_px: [1280, 720],
            viewport_rect: Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0)),
            tool: Tool::Select,
            inspector_tab: InspectorTab::Info,
            logo: Some(load_logo(&cc.egui_ctx)),
            texture_dirs: Vec::new(),
            flying: false,
            snap_on_release: false,
        }
    }

    /// Applies preferences that live outside the settings struct: language, single window.
    fn sync_preferences(&mut self, ctx: &egui::Context) {
        if self.applied_language != Some(self.settings.language) {
            i18n::set(self.settings.language);
            self.applied_language = Some(self.settings.language);
        }
        let listen = self.settings.single_window && self.capture.is_none();
        if listen != self.instance.is_some() {
            self.instance = if listen { instance::listen(ctx) } else { None };
        }
        let received: Vec<PathBuf> = self.instance.as_ref().map(|s| s.files.try_iter().collect()).unwrap_or_default();
        if let Some(path) = received.into_iter().last() {
            self.open(path, ctx);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    // --- Files ------------------------------------------------------------------------------

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        if loader::is_environment(&path) {
            self.settings.add_environment(&path.to_string_lossy());
            self.settings.shading = ShadingMode::Rendered;
            return;
        }
        if !loader::is_supported(&path) {
            let ext = path
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_else(|| tr("This file").into());
            let text = trf(
                "{ext} isn't supported yet. Supported: glTF, GLB, FBX, OBJ, STL, and .hdr / .exr environments.",
                &[("ext", &ext)],
            );
            self.show_toast(ctx, text, true);
            return;
        }
        if self.info.as_ref().is_none_or(|i| i.path != path) {
            self.texture_dirs.clear();
        }
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        let thread_path = path.clone();
        let texture_dirs = self.texture_dirs.clone();
        std::thread::spawn(move || {
            let result = loader::load_with(&thread_path, &texture_dirs);
            let _ = tx.send(result);
            repaint.request_repaint();
        });
        self.loading = Some(Loading { path, rx, started: Instant::now() });
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        let picked = rfd::FileDialog::new()
            .set_title(tr("Open 3D file"))
            .add_filter(tr("3D models"), loader::SUPPORTED_EXTENSIONS)
            .add_filter("glTF", &["glb", "gltf"])
            .add_filter("FBX", &["fbx"])
            .add_filter("Wavefront OBJ", &["obj"])
            .add_filter("STL", &["stl"])
            .pick_file();
        if let Some(path) = picked {
            self.open(path, ctx);
        }
    }

    fn open_hdri_dialog(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title(tr("Load HDRI environment"))
            .add_filter("HDRI (.hdr, .exr)", loader::ENVIRONMENT_EXTENSIONS)
            .pick_file();
        if let Some(path) = picked {
            self.settings.add_environment(&path.to_string_lossy());
        }
    }

    fn export_image(&mut self, ctx: &egui::Context) {
        let stem = self.info.as_ref().map_or("render".to_string(), |i| {
            Path::new(&i.file_name).file_stem().map_or(i.file_name.clone(), |s| s.to_string_lossy().into_owned())
        });
        let Some(path) = rfd::FileDialog::new()
            .set_title(tr("Save image"))
            .add_filter(tr("PNG image"), &["png"])
            .set_file_name(format!("{stem}.png"))
            .save_file()
        else {
            return;
        };
        let path = if path.extension().is_none() { path.with_extension("png") } else { path };
        let result = self.export_to(&path);
        match result {
            Ok(()) => self.show_toast(ctx, trf("Saved {name}", &[("name", &file_name(&path))]), false),
            Err(e) => self.show_toast(ctx, e, true),
        }
        ctx.request_repaint();
    }

    /// Saves the current view as a PNG, without UI and selection outlines.
    fn export_to(&mut self, path: &Path) -> Result<(), String> {
        let Some(renderer) = &mut self.renderer else { return Err(tr("Couldn't render the image").into()) };
        // 4x MSAA color + depth at 4096 px is already ~150 MB each; keep exports under that.
        const MAX_SIDE: f32 = 4096.0;
        let [w, h] = self.viewport_px;
        let scale = (self.settings.export_scale.clamp(1, 4) as f32).min(MAX_SIDE / w.max(h).max(1) as f32);
        let size = [(w as f32 * scale).round() as u32, (h as f32 * scale).round() as u32];

        let mut settings = self.settings.clone();
        settings.show_grid = settings.show_overlays && settings.export_grid;
        settings.show_wire_overlay &= settings.show_overlays;
        settings.show_outline &= settings.show_overlays;
        settings.show_non_manifold &= settings.show_overlays;
        settings.show_open_edges &= settings.show_overlays;
        settings.show_overlapping &= settings.show_overlays;
        if settings.export_transparent {
            settings.env_background = false;
        }
        let (grid_cell, grid_fade, grid_axis) = grid_params(&self.camera);
        // Hide the selection; the next viewport frame sets it again.
        renderer.set_selection(&vec![0; self.selection.selected.len()]);
        let input = FrameInput {
            view: self.camera.view_matrix(),
            proj: self.camera.projection(size[0] as f32 / size[1] as f32),
            eye: self.camera.eye(),
            cam_back: self.camera.back(),
            ortho: self.camera.ortho,
            grid_cell,
            grid_fade,
            grid_axis,
            settings: &settings,
            pick: None,
            transparent: settings.export_transparent,
        };
        renderer.render(None, size, &input);
        match renderer.read_pixels() {
            Some(([w, h], pixels)) => image::save_buffer(path, &pixels, w, h, image::ColorType::Rgba8)
                .map_err(|e| trf("Couldn't save the image: {error}", &[("error", &e)])),
            None => Err(tr("Couldn't render the image").to_string()),
        }
    }

    fn preferences(&mut self, ctx: &egui::Context) {
        let mut open = self.show_prefs;
        egui::Window::new(tr("Preferences"))
            .id(egui::Id::new("preferences"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let s = &mut self.settings;
                widgets::section(ui, "Navigation");
                egui::ComboBox::from_label(tr("Style"))
                    .selected_text(s.navigation.label())
                    .show_ui(ui, |ui| {
                        for nav in Navigation::ALL {
                            ui.selectable_value(&mut s.navigation, nav, nav.label());
                        }
                    });
                ui.add_space(6.0);
                widgets::section(ui, "Interface");
                egui::ComboBox::from_label(tr("Language"))
                    .selected_text(s.language.label())
                    .show_ui(ui, |ui| {
                        for lang in Language::ALL {
                            ui.selectable_value(&mut s.language, lang, lang.label());
                        }
                    });
                ui.add_space(6.0);
                widgets::section(ui, "Rendered");
                let env_name = |e: &Environment| match e {
                    Environment::Preset(p) => tr(p.label()).to_string(),
                    Environment::File(path) => file_name(Path::new(path)),
                };
                egui::ComboBox::from_label(tr("Default environment"))
                    .selected_text(env_name(&s.default_environment))
                    .show_ui(ui, |ui| {
                        let presets = environment::Preset::ALL.into_iter().map(Environment::Preset);
                        let files = s.custom_environments.iter().cloned().map(Environment::File);
                        for env in presets.chain(files).collect::<Vec<_>>() {
                            let name = env_name(&env);
                            ui.selectable_value(&mut s.default_environment, env, name);
                        }
                    });
                ui.add_space(6.0);
                widgets::section(ui, "Files");
                ui.checkbox(&mut s.single_window, tr("Open files in the same window")).on_hover_text(tr(
                    "Opening a file from Explorer loads it in the window that's already open instead of starting a new one",
                ));
                // Windows only lets the user pick default apps; open that page on our entry.
                if ui
                    .button(tr("Choose file types…"))
                    .on_hover_text(tr("Opens Windows Default apps, where Poly Loupe can open .glb, .gltf, .fbx, .obj and .stl"))
                    .clicked()
                {
                    let _ = std::process::Command::new("explorer.exe")
                        .arg("ms-settings:defaultapps?registeredAppMachine=Poly%20Loupe")
                        .spawn();
                }
                ui.add_space(6.0);
                widgets::section(ui, "Viewport");
                ui.checkbox(&mut s.vsync, tr("V-Sync"))
                    .on_hover_text(tr("Off: frames aren't capped to the monitor refresh rate"));
                ui.checkbox(&mut s.show_fps, tr("Frame rate"))
                    .on_hover_text(tr("Redraws continuously and shows FPS in the status bar"));
                ui.add_space(6.0);
                widgets::section(ui, "Image Export");
                ui.horizontal(|ui| {
                    ui.label(tr("Resolution"));
                    widgets::segmented(ui, &mut s.export_scale, &[(1, "1×"), (2, "2×"), (4, "4×")]);
                })
                .response
                .on_hover_text(tr("Multiplies the viewport size, up to 4096 px on the long side"));
                ui.checkbox(&mut s.export_transparent, tr("Transparent background"));
                ui.checkbox(&mut s.export_grid, tr("Include the floor grid"));
            });
        self.show_prefs = open;
    }

    /// First run: pick the navigation you already know. Shown once.
    fn welcome(&mut self, ctx: &egui::Context) {
        let capture_popover = self.capture.as_ref().and_then(|c| c.opts.popover.as_deref());
        if (self.settings.onboarded || self.capture.is_some()) && capture_popover != Some("welcome") {
            return;
        }
        egui::Modal::new(egui::Id::new("welcome")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(tr("Welcome to Poly Loupe")).size(17.0).color(theme::TEXT));
            ui.add_space(4.0);
            ui.label(
                RichText::new(tr("How do you move around in 3D? Pick the app your hands already know; you can change it later in Preferences."))
                    .color(theme::TEXT_DIM),
            );
            ui.add_space(10.0);
            for nav in Navigation::ALL {
                let selected = self.settings.navigation == nav;
                let text = RichText::new(nav.label()).color(if selected { theme::ON_ACCENT } else { theme::TEXT });
                let button = egui::Button::new(text)
                    .fill(if selected { theme::ACCENT } else { theme::WIDGET })
                    .min_size(vec2(ui.available_width(), 28.0));
                if ui.add(button).clicked() {
                    self.settings.navigation = nav;
                }
            }
            ui.add_space(6.0);
            let hints: Vec<String> = self
                .settings
                .navigation
                .help()
                .iter()
                .map(|(keys, action)| format!("{}: {}", key_names(keys), tr(action)))
                .collect();
            ui.label(RichText::new(hints.join("   ·   ")).size(11.0).color(theme::TEXT_DIM));
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let start = egui::Button::new(RichText::new(tr("Start")).color(theme::ON_ACCENT))
                    .fill(theme::ACCENT)
                    .min_size(vec2(96.0, 28.0));
                if ui.add(start).clicked() {
                    self.settings.onboarded = true;
                }
            });
        });
    }

    fn poll_loading(&mut self, ctx: &egui::Context) {
        let Some(loading) = &self.loading else { return };
        let Ok(result) = loading.rx.try_recv() else { return };
        let loading = self.loading.take().expect("checked above");
        let mut scene = match result {
            Ok(scene) => scene,
            Err(err) => {
                let text = trf("Couldn't open {name}: {error}", &[("name", &file_name(&loading.path)), ("error", &err)]);
                self.show_toast(ctx, text, true);
                return;
            }
        };
        if let Some(r) = &mut self.renderer {
            r.upload_scene(&scene);
        }
        let has_morphs = scene.meshes.iter().any(|m| !m.rig.morph_targets.is_empty());
        let animation = std::mem::take(&mut scene.animation);
        self.anim = (animation.is_animated() || has_morphs).then(|| AnimPlayer::new(animation, &scene.meshes));
        self.anim_dirty = true;
        self.refresh_anim_keys();
        self.map_thumbs.clear();
        for m in &scene.materials {
            for map in m.maps() {
                let key = (map.image, map.channel);
                if self.map_thumbs.contains_key(&key) {
                    continue;
                }
                if let Some(img) = scene.images.get(map.image) {
                    let (size, pixels) = img.thumbnail(128, map.channel);
                    let image = egui::ColorImage::from_rgba_unmultiplied(size, &pixels);
                    let name = format!("map_{}_{:?}", map.image, map.channel);
                    self.map_thumbs.insert(key, ctx.load_texture(name, image, TextureOptions::LINEAR));
                }
            }
        }
        let file_name = file_name(&loading.path);
        self.camera.reset(&scene.bounds);
        let objects: Vec<ObjectMeta> = scene
            .meshes
            .iter()
            .map(|m| ObjectMeta {
                name: m.name.clone(),
                triangles: m.triangle_count(),
                vertices: m.positions.len(),
                bounds: m.bounds.transformed(&m.transform),
                material: m.material.min(scene.materials.len() - 1),
            })
            .collect();
        self.selection.reset(objects.len());
        self.measures.clear();
        self.cancel_measure();
        self.visible = vec![true; objects.len()];
        self.pass_override = vec![None; objects.len()];
        let missing: Vec<String> = scene
            .images
            .iter()
            .filter_map(|i| i.name.strip_suffix(loader::MISSING_SUFFIX).map(str::to_string))
            .collect();
        let missing_count = missing.len();
        self.info = Some(SceneInfo {
            path: loading.path.clone(),
            missing,
            file_name: file_name.clone(),
            materials: scene
                .materials
                .iter()
                .map(|m| MaterialMeta { name: m.name.clone(), maps: m.maps() })
                .collect(),
            images: scene.images.iter().map(|i| (i.name.clone(), i.width, i.height)).collect(),
            objects,
            vertices: scene.source_vertex_count,
            triangles: scene.triangle_count(),
            load_time: loading.started.elapsed(),
            qa: None,
        });
        self.settings.push_recent(&loading.path.to_string_lossy());
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{file_name} — {APP_NAME}")));
        // Missing textures get a banner with a fix; other warnings a plain toast.
        let other: Vec<&String> = scene.warnings.iter().filter(|w| !w.starts_with(&missing_prefix())).collect();
        if missing_count > 0 {
            let text = trf("{n} textures not found. The model is shown without them.", &[("n", &missing_count)]);
            self.show_banner(ctx, text, ToastAction::ShowMaterials);
        } else if let Some(first) = other.first() {
            let more = other.len() - 1;
            let text = if more > 0 { trf("{first} (+{more} more)", &[("first", first), ("more", &more)]) } else { (*first).clone() };
            self.show_toast(ctx, text, false);
        }
        // The CPU copy goes to the mesh analysis, then is dropped: the GPU holds everything
        // the viewport needs. A newer load replaces `qa_rx`, so stale results are ignored.
        let (tx, rx) = channel();
        self.qa_rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let (report, marks) = qa::analyze(&scene.meshes);
            log::info!("mesh analysis in {:?}: {report:?}", started.elapsed());
            let instances = scene.meshes.iter().zip(&marks).map(|(m, k)| render::marker_instances(m, k)).collect();
            if tx.send((report, instances)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    fn cancel_measure(&mut self) {
        self.measure_start = None;
        self.measure_hover = None;
        self.measure_hover_px = None;
    }

    fn poll_qa(&mut self) {
        let Some(rx) = &self.qa_rx else { return };
        let Ok((report, instances)) = rx.try_recv() else { return };
        self.qa_rx = None;
        if let Some(r) = &mut self.renderer {
            r.set_markers(&instances);
        }
        if let Some(info) = &mut self.info {
            info.qa = Some(report);
        }
    }

    /// Loads the selected environment on a worker thread when the Rendered mode needs it.
    fn update_environment(&mut self, ctx: &egui::Context) {
        if let Some((env, rx)) = &self.env_loading {
            if let Ok(result) = rx.try_recv() {
                let env = env.clone();
                self.env_loading = None;
                match result {
                    Ok(image) => {
                        if let Some(r) = &self.renderer {
                            r.set_environment(&image);
                        }
                        if let Environment::File(path) = &env {
                            self.thumbs.add_environment(ctx, path, &image);
                        }
                        self.env_loaded = Some(env);
                    }
                    Err(e) => {
                        self.show_toast(ctx, e, true);
                        self.settings.environment = self.env_loaded.clone().unwrap_or(Environment::Preset(environment::Preset::Forest));
                    }
                }
                ctx.request_repaint();
            }
            return;
        }
        let wanted = &self.settings.environment;
        if self.settings.shading != ShadingMode::Rendered || self.env_loaded.as_ref() == Some(wanted) {
            return;
        }
        let env = wanted.clone();
        let job = env.clone();
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = match job {
                Environment::Preset(p) => Ok(environment::load(p)),
                Environment::File(path) => loader::load_environment(Path::new(&path)),
            };
            let _ = tx.send(result);
            repaint.request_repaint();
        });
        self.env_loading = Some((env, rx));
    }

    fn refresh_anim_keys(&mut self) {
        self.anim_keys.clear();
        let Some(p) = &self.anim else { return };
        let Some(clip) = p.clip.and_then(|c| p.data.clips.get(c)) else { return };
        let fps = p.fps();
        let mut keys: Vec<i32> = clip
            .channels
            .iter()
            .flat_map(|c| c.times.iter().map(|t| ((t - clip.start) * fps).round() as i32))
            .collect();
        keys.sort_unstable();
        keys.dedup();
        // Baked animation (a key on every frame) would just draw a solid row of diamonds.
        let frames = (clip.duration * fps).round() as usize + 1;
        let baked = keys.len() > 12 && keys.len() * 2 >= frames;
        if !baked && keys.len() <= 500 {
            self.anim_keys = keys;
        }
    }

    /// Advances playback and pushes the pose to the renderer when it changed.
    fn update_animation(&mut self, ctx: &egui::Context) {
        let Some(p) = &mut self.anim else { return };
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        let moved = p.tick(dt);
        if moved || self.anim_dirty {
            let pose = p.pose();
            if let Some(r) = &mut self.renderer {
                r.set_object_transforms(&pose.object_transforms);
                r.set_joints(&pose.joints);
                for (i, positions, normals) in &pose.morphed {
                    r.update_mesh_geometry(*i, positions, normals);
                }
            }
            self.anim_dirty = false;
        }
        if p.playing {
            ctx.request_repaint();
        }
    }

    fn timeline_action(&mut self, action: TimelineAction) {
        let Some(p) = &mut self.anim else { return };
        match action {
            TimelineAction::TogglePlay => {
                if !p.playing && !p.looping && p.frame() >= p.last_frame() {
                    p.time = 0.0;
                }
                p.playing = !p.playing;
            }
            TimelineAction::JumpStart => p.set_frame(0),
            TimelineAction::JumpEnd => {
                let last = p.last_frame();
                p.set_frame(last);
            }
            TimelineAction::Step(d) => {
                let (f, last) = (p.frame() + d, p.last_frame());
                p.set_frame(if f < 0 { last } else if f > last { 0 } else { f });
            }
            TimelineAction::SetFrame(f) => p.set_frame(f),
            TimelineAction::SetClip(c) => {
                p.clip = Some(c);
                p.time = p.time.min(p.duration());
            }
            TimelineAction::SetSpeed(s) => p.speed = s,
            TimelineAction::ToggleLoop => p.looping = !p.looping,
        }
        self.anim_dirty = true;
        if matches!(action, TimelineAction::SetClip(_)) {
            self.refresh_anim_keys();
        }
    }

    fn timeline(&mut self, ui: &mut Ui) {
        let Some(p) = &self.anim else { return };
        let view = TimelineView {
            clips: p.data.clips.iter().map(|c| c.name.as_str()).collect(),
            clip: p.clip,
            frame: p.frame(),
            last_frame: p.last_frame(),
            playing: p.playing,
            speed: p.speed,
            looping: p.looping,
            fps: p.fps(),
            keys: &self.anim_keys,
        };
        let actions = timeline::show(ui, &view);
        for a in actions {
            self.timeline_action(a);
        }
    }

    fn show_toast(&mut self, ctx: &egui::Context, text: String, error: bool) {
        log::warn!("{text}");
        let now = ctx.input(|i| i.time);
        self.toast = Some(Toast { text, until: now + 6.0, error, action: None });
        ctx.request_repaint_after(Duration::from_secs_f32(6.1));
    }

    fn show_banner(&mut self, ctx: &egui::Context, text: String, action: ToastAction) {
        log::warn!("{text}");
        let now = ctx.input(|i| i.time);
        self.toast = Some(Toast { text, until: now, error: false, action: Some(action) });
    }

    // --- Selection & visibility -------------------------------------------------------------

    fn visible_bounds(&self, only_selected: bool) -> Aabb {
        let mut b = Aabb::EMPTY;
        if let Some(info) = &self.info {
            for (i, o) in info.objects.iter().enumerate() {
                let sel = self.selection.selected.get(i).copied().unwrap_or(false);
                if self.visible[i] && (!only_selected || sel) {
                    b.union(&o.bounds);
                }
            }
        }
        b
    }

    fn frame_all(&mut self) {
        let b = self.visible_bounds(false);
        self.camera.frame(&b, true);
    }

    fn frame_selected(&mut self) {
        if self.selection.any() {
            let b = self.visible_bounds(true);
            self.camera.frame(&b, true);
        } else {
            self.frame_all();
        }
    }

    /// Channel view: on the selection when there is one, otherwise on the whole scene.
    fn apply_channel(&mut self, action: ChannelAction) {
        let targets: Vec<usize> = (0..self.pass_override.len()).filter(|&i| self.selection.selected[i]).collect();
        match action {
            ChannelAction::ClearOverrides => self.pass_override.fill(None),
            ChannelAction::Off if targets.is_empty() => {
                if self.settings.color == ColorMode::Texture {
                    self.settings.color = ColorMode::Material;
                }
            }
            ChannelAction::Off => {
                for i in targets {
                    self.pass_override[i] = None;
                }
            }
            ChannelAction::Show(pass) if targets.is_empty() => {
                self.settings.color = ColorMode::Texture;
                self.settings.texture_pass = pass;
                // The global color mode is a Solid setting.
                self.settings.shading = ShadingMode::Solid;
            }
            ChannelAction::Show(pass) => {
                for i in targets {
                    self.pass_override[i] = Some(pass);
                }
                if self.settings.shading == ShadingMode::Wireframe {
                    self.settings.shading = ShadingMode::Solid;
                }
            }
        }
    }


    fn hide_selected(&mut self, selected: bool) {
        for (i, v) in self.visible.iter_mut().enumerate() {
            if self.selection.selected[i] == selected {
                *v = false;
            }
        }
        // Hidden objects can't stay selected.
        for (i, s) in self.selection.selected.iter_mut().enumerate() {
            if !self.visible[i] {
                *s = false;
            }
        }
        if self.selection.active.is_some_and(|a| !self.visible[a]) {
            self.selection.active = None;
        }
    }

    fn reveal_all(&mut self) {
        for (i, v) in self.visible.iter_mut().enumerate() {
            if !*v {
                *v = true;
                // Blender selects what it reveals.
                self.selection.selected[i] = true;
            }
        }
    }

    // --- Input ------------------------------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || self.flying {
            return;
        }
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        for (key, tool) in [(Key::Q, Tool::Select), (Key::O, Tool::Orbit), (Key::G, Tool::Pan), (Key::M, Tool::Measure)] {
            if pressed(Modifiers::NONE, key) {
                self.tool = tool;
            }
        }
        if self.settings.navigation.f_frames() && pressed(Modifiers::NONE, Key::F) {
            self.frame_selected();
        }

        if pressed(Modifiers::COMMAND, Key::O) {
            self.open_dialog(ctx);
        }
        if pressed(Modifiers::NONE, Key::F12) {
            self.export_image(ctx);
        }
        if pressed(Modifiers::COMMAND, Key::Comma) {
            self.show_prefs = !self.show_prefs;
        }
        if pressed(Modifiers::COMMAND, Key::Q) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if pressed(Modifiers::NONE, Key::F11) {
            let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
        }
        if pressed(Modifiers::NONE, Key::Home) {
            self.frame_all();
        }
        if pressed(Modifiers::NONE, Key::Period) {
            self.frame_selected();
        }
        if pressed(Modifiers::NONE, Key::N) {
            self.settings.show_sidebar = !self.settings.show_sidebar;
        }
        if pressed(Modifiers::NONE, Key::C) {
            self.cycle_channel(true);
        }
        // Measure: Esc drops the point being placed, Delete clears every measurement.
        if self.measure_start.is_some() && pressed(Modifiers::NONE, Key::Escape) {
            self.cancel_measure();
        }
        if !self.measures.is_empty() && (pressed(Modifiers::NONE, Key::Delete) || pressed(Modifiers::NONE, Key::Backspace)) {
            self.measures.clear();
            self.cancel_measure();
        }
        if pressed(Modifiers::SHIFT, Key::C) {
            self.cycle_channel(false);
        }

        // Playback (Blender keymap).
        if self.anim.as_ref().is_some_and(|a| a.has_clips()) {
            if pressed(Modifiers::NONE, Key::Space) {
                self.timeline_action(TimelineAction::TogglePlay);
            }
            if pressed(Modifiers::SHIFT, Key::ArrowLeft) {
                self.timeline_action(TimelineAction::JumpStart);
            }
            if pressed(Modifiers::SHIFT, Key::ArrowRight) {
                self.timeline_action(TimelineAction::JumpEnd);
            }
            if pressed(Modifiers::NONE, Key::ArrowLeft) {
                self.timeline_action(TimelineAction::Step(-1));
            }
            if pressed(Modifiers::NONE, Key::ArrowRight) {
                self.timeline_action(TimelineAction::Step(1));
            }
        }

        // Selection and visibility (Blender keymap).
        if self.info.is_some() {
            if pressed(Modifiers::ALT, Key::A) {
                self.selection.clear();
            }
            if pressed(Modifiers::NONE, Key::A) {
                self.selection.all(&self.visible);
            }
            if pressed(Modifiers::ALT, Key::H) {
                self.reveal_all();
            }
            if pressed(Modifiers::SHIFT, Key::H) {
                self.hide_selected(false);
            }
            if pressed(Modifiers::NONE, Key::H) {
                self.hide_selected(true);
            }
        }

        // Shading (Blender 2.8+ keymap).
        if pressed(Modifiers::SHIFT | Modifiers::ALT, Key::Z) {
            self.settings.show_overlays = !self.settings.show_overlays;
        }
        if pressed(Modifiers::ALT, Key::Z) {
            self.settings.toggle_xray();
        }
        if pressed(Modifiers::SHIFT, Key::Z) {
            self.settings.shading = match self.settings.shading {
                ShadingMode::Wireframe => ShadingMode::Solid,
                _ => ShadingMode::Wireframe,
            };
        }
        if !self.pie.is_open() && pressed(Modifiers::NONE, Key::Z) {
            let (pos, time) = ctx.input(|i| (i.pointer.hover_pos(), i.time));
            let center = pos.unwrap_or_else(|| ctx.content_rect().center());
            self.pie.open(center, time);
        }

        // Numpad-style views. Top-row digits work too, for laptops without a numpad.
        for (key, view) in [(Key::Num1, AxisView::Front), (Key::Num3, AxisView::Right), (Key::Num7, AxisView::Top)] {
            if pressed(Modifiers::COMMAND, key) {
                self.camera.set_axis_view(view.opposite());
            } else if pressed(Modifiers::NONE, key) {
                self.camera.set_axis_view(view);
            }
        }
        if pressed(Modifiers::NONE, Key::Num9) {
            if let Some(v) = self.camera.axis_view {
                self.camera.set_axis_view(v.opposite());
            }
        }
        if pressed(Modifiers::NONE, Key::Num5) {
            self.camera.toggle_ortho();
        }
        let step = 15f32.to_radians() / 0.0075; // one Blender orbit step, in orbit pixels
        for (key, dx, dy) in [
            (Key::Num4, -step, 0.0),
            (Key::Num6, step, 0.0),
            (Key::Num8, 0.0, -step),
            (Key::Num2, 0.0, step),
        ] {
            if pressed(Modifiers::NONE, key) {
                self.camera.orbit(dx, dy);
            }
        }
    }

    fn navigate(&mut self, ui: &Ui, response: &egui::Response, viewport: Rect) {
        let ppp = ui.ctx().pixels_per_point();
        let (mods, scroll, pinch, dt) =
            ui.input(|i| (i.modifiers, i.smooth_scroll_delta, i.zoom_delta(), i.stable_dt.min(0.05)));
        let nav = self.settings.navigation;
        let drag = DragInput {
            left: response.dragged_by(PointerButton::Primary),
            middle: response.dragged_by(PointerButton::Middle),
            right: response.dragged_by(PointerButton::Secondary),
            alt: mods.alt,
            shift: mods.shift,
            ctrl: mods.ctrl,
        };
        // The rail's tools turn a plain left drag into a camera move.
        let tool_gesture = match self.tool {
            _ if !(drag.left && !mods.any()) => None,
            Tool::Select => None,
            Tool::Orbit => Some(Gesture::Orbit),
            Tool::Pan => Some(Gesture::Pan),
            Tool::Zoom => Some(Gesture::Zoom),
            Tool::Measure => None,
        };
        let gesture = nav.gesture(drag).or(tool_gesture);
        let d = response.drag_delta() * ppp;
        match gesture {
            Some(Gesture::Orbit) => {
                self.camera.orbit(d.x, d.y);
                if nav == Navigation::ZBrush {
                    self.snap_on_release |= mods.shift;
                }
            }
            Some(Gesture::Pan) => self.camera.pan(d.x, d.y, viewport.height() * ppp),
            // Right/down zooms in for the drag-to-zoom presets, like Maya's dolly.
            Some(Gesture::Zoom) => self.camera.zoom((d.x - d.y) * 0.02),
            Some(Gesture::Look) => self.camera.look(d.x, d.y),
            None => {}
        }
        if self.snap_on_release && gesture.is_none() {
            self.snap_on_release = false;
            self.camera.snap_to_axis();
        }

        // Unity/Unreal fly: WASD (Q/E down/up) while looking with the right button.
        // RMB held counts even before the pointer moves, so WASD works from the first frame.
        let rmb_held = response.is_pointer_button_down_on() && ui.input(|i| i.pointer.secondary_down());
        self.flying = nav == Navigation::GameEngine && !mods.alt && rmb_held;
        if self.flying {
            let (fwd, right, up) = ui.input(|i| {
                let axis = |pos: Key, neg: Key| i.key_down(pos) as i32 as f32 - i.key_down(neg) as i32 as f32;
                (axis(Key::W, Key::S), axis(Key::D, Key::A), axis(Key::E, Key::Q))
            });
            if fwd != 0.0 || right != 0.0 || up != 0.0 {
                let speed = if mods.shift { 3.0 } else { 1.0 } * dt;
                self.camera.fly(fwd * speed, right * speed, up * speed);
                ui.ctx().request_repaint();
            }
        }
        if response.hovered() {
            if scroll.y != 0.0 {
                self.camera.zoom(scroll.y / 50.0);
            }
            if pinch != 1.0 {
                self.camera.zoom(pinch.ln() / (1.0f32 / 0.85).ln());
            }
        }
        let to_px = |pos: Pos2| {
            let p = (pos - viewport.min) * ppp;
            [p.x.max(0.0) as u32, p.y.max(0.0) as u32]
        };
        // Click to select or place a measurement point (not with Alt, which is navigation in
        // most presets; not on a drag).
        if response.clicked_by(PointerButton::Primary) && !mods.alt && self.info.is_some() {
            if let Some(pos) = response.interact_pointer_pos() {
                let purpose = if self.tool == Tool::Measure {
                    PickPurpose::MeasureClick
                } else {
                    PickPurpose::Select { extend: mods.shift }
                };
                self.pending_pick = Some((to_px(pos), purpose));
            }
        } else if self.tool == Tool::Measure && self.measure_start.is_some() && self.pending_pick.is_none() {
            // Follow the cursor with a pick only when it moves: each pick waits for the GPU.
            let hover = response.hover_pos().map(to_px);
            if hover.is_some() && hover != self.measure_hover_px {
                self.measure_hover_px = hover;
                self.pending_pick = hover.map(|p| (p, PickPurpose::MeasureHover));
            }
        }
        if self.tool == Tool::Measure && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    // --- Layout -----------------------------------------------------------------------------

    fn viewport(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let rect = ui.max_rect();
        self.viewport_rect = rect;
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let ctx = ui.ctx().clone();

        if self.info.is_none() && self.loading.is_none() {
            self.empty_state(ui, rect);
        } else {
            if !self.pie.is_open() {
                self.navigate(ui, &response, rect);
            }
            let dt = ui.input(|i| i.stable_dt).min(0.05);
            if self.camera.update(dt) {
                ctx.request_repaint();
            }
            self.render_viewport(ui, frame, rect);
            self.draw_measures(ui, rect);

            let overlays = self.settings.show_overlays;
            if overlays {
                self.hud(ui, rect);
            }
            let gizmo_shown = overlays && self.settings.show_gizmo;
            if self.loading.is_none() {
                self.channel_strip(&ctx, rect, gizmo_shown);
            }
            if gizmo_shown {
                let center = pos2(rect.right() - gizmo::WIDTH * 0.5 - 16.0, rect.top() + gizmo::WIDTH * 0.5 + 16.0);
                let ppp = ctx.pixels_per_point();
                for action in gizmo::show(ui, center, &self.camera) {
                    match action {
                        GizmoAction::Orbit(d) => self.camera.orbit(d.x * ppp, d.y * ppp),
                        GizmoAction::AxisView(v) => self.camera.set_axis_view(v),
                    }
                }
            }
            self.viewport_toolbar(&ctx, rect);
            if let Some(loading) = &self.loading {
                shell::loading_overlay(ui, rect, &file_name(&loading.path));
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }

        let hovering_files = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if hovering_files {
            shell::drop_overlay(ui, rect);
        }
    }

    fn render_viewport(&mut self, ui: &mut Ui, frame: &mut eframe::Frame, rect: Rect) {
        let (Some(renderer), Some(rs)) = (&mut self.renderer, frame.wgpu_render_state()) else {
            ui.painter().rect_filled(rect, 0.0, theme::VIEWPORT);
            return;
        };
        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp).round().max(1.0) as u32, (rect.height() * ppp).round().max(1.0) as u32];
        let aspect = size[0] as f32 / size[1] as f32;

        self.viewport_px = size;
        let (grid_cell, grid_fade, grid_axis) = grid_params(&self.camera);

        renderer.set_selection(&self.selection.states());
        let overrides: Vec<u32> = self
            .pass_override
            .iter()
            .map(|p| p.map_or(0, |p| TexturePass::ALL.iter().position(|q| *q == p).unwrap_or(0) as u32 + 1))
            .collect();
        renderer.set_pass_overrides(&overrides);
        renderer.set_visibility(&self.visible);

        let mut effective = self.settings.clone();
        if !effective.show_overlays {
            effective.show_grid = false;
            effective.show_wire_overlay = false;
            effective.show_outline = false;
            effective.show_non_manifold = false;
            effective.show_open_edges = false;
            effective.show_overlapping = false;
        }
        let pick = self.pending_pick.take();
        let input = FrameInput {
            view: self.camera.view_matrix(),
            proj: self.camera.projection(aspect),
            eye: self.camera.eye(),
            cam_back: self.camera.back(),
            ortho: self.camera.ortho,
            grid_cell,
            grid_fade,
            grid_axis,
            settings: &effective,
            pick: pick.map(|p| p.0),
            transparent: false,
        };
        let texture = {
            let mut egui_renderer = rs.renderer.write();
            renderer.render(Some(&mut egui_renderer), size, &input)
        };
        if let Some(texture) = texture {
            ui.painter().image(texture, rect, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        }

        let hit = renderer.picked.take();
        let point = renderer.picked_point.take().flatten();
        match (pick.map(|p| p.1), hit) {
            (Some(PickPurpose::Select { extend }), Some(hit)) => {
                self.selection.click(hit, extend);
                ui.ctx().request_repaint();
            }
            (Some(PickPurpose::MeasureClick), _) => {
                if let Some(p) = point {
                    match self.measure_start.take() {
                        None => self.measure_start = Some(p),
                        Some(a) => self.measures.push([a, p]),
                    }
                    self.measure_hover = None;
                    self.measure_hover_px = None;
                }
                ui.ctx().request_repaint();
            }
            (Some(PickPurpose::MeasureHover), _) => {
                self.measure_hover = point;
                ui.ctx().request_repaint();
            }
            _ => {}
        }
    }

    fn drive_capture(&mut self, ctx: &egui::Context) {
        let mut pending_channel = None;
        let Some(cap) = &mut self.capture else { return };
        ctx.request_repaint();
        if self.loading.is_some() || self.env_loading.is_some() {
            return;
        }
        // Hold the shot until the mesh analysis is in, so its markers are in the picture.
        if cap.frames > 2 && self.qa_rx.is_some() {
            return;
        }
        cap.frames += 1;
        if let (4, Some([_, b])) = (cap.frames, cap.opts.measure) {
            self.pending_pick = Some((b, PickPurpose::MeasureClick));
        }
        if cap.frames == 2 {
            if let Some(view) = cap.opts.view {
                self.camera.set_axis_view(view);
            }
            if cap.opts.ortho {
                self.camera.ortho = true;
            }
            match cap.opts.popover.as_deref() {
                Some("preferences") => self.show_prefs = true,
                Some("materials") => {
                    self.settings.show_sidebar = true;
                    self.inspector_tab = InspectorTab::Materials;
                }
                Some("scene") => {
                    self.settings.show_sidebar = true;
                    self.inspector_tab = InspectorTab::Scene;
                }
                Some("shading") => {
                    self.settings.show_sidebar = true;
                    self.inspector_tab = InspectorTab::Shading;
                }
                _ => {}
            }
            if cap.opts.pie {
                self.pie.open(ctx.content_rect().center(), ctx.input(|i| i.time));
            }
            if let Some([a, _]) = cap.opts.measure {
                self.tool = Tool::Measure;
                self.pending_pick = Some((a, PickPurpose::MeasureClick));
            }
            if let Some(p) = cap.opts.click {
                self.pending_pick = Some((p, PickPurpose::Select { extend: false }));
            }
            if let Some(i) = cap.opts.select {
                if i < self.selection.selected.len() {
                    self.selection.click(Some(i), false);
                }
            }
            pending_channel = cap.opts.channel;
            if let Some(p) = &mut self.anim {
                if let Some(c) = cap.opts.clip {
                    if c < p.data.clips.len() {
                        p.clip = Some(c);
                    }
                }
                if let Some(f) = cap.opts.frame {
                    p.set_frame(f);
                }
                self.anim_dirty = true;
            }
        }
        // Frame-rate captures run longer so the average settles.
        let settle = if self.settings.show_fps { 400 } else { 40 };
        if cap.frames == settle && !cap.requested {
            cap.requested = true;
            if let Some(out) = cap.opts.export.clone() {
                match self.export_to(&out) {
                    Ok(()) => println!("exported {}", out.display()),
                    Err(e) => eprintln!("export failed: {e}"),
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = shot {
            let [w, h] = image.size;
            let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
            match image::save_buffer(&cap.opts.out, &bytes, w as u32, h as u32, image::ColorType::Rgba8) {
                Ok(()) => println!("saved {}", cap.opts.out.display()),
                Err(e) => eprintln!("capture failed: {e}"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(pass) = pending_channel {
            self.apply_channel(ChannelAction::Show(pass));
        }
    }
}

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.sync_preferences(&ctx);
        if let Some(path) = self.pending_open.take() {
            self.open(path, &ctx);
        }
        self.poll_loading(&ctx);
        self.poll_qa();
        self.update_environment(&ctx);

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if let Some(path) = dropped.into_iter().next() {
            self.open(path, &ctx);
        }

        if self.applied_vsync != Some(self.settings.vsync) {
            frame.set_wgpu_surface_config(egui_wgpu::SurfaceConfig {
                present_mode: if self.settings.vsync {
                    egui_wgpu::wgpu::PresentMode::AutoVsync
                } else {
                    egui_wgpu::wgpu::PresentMode::AutoNoVsync
                },
                desired_maximum_frame_latency: Some(1),
            });
            self.applied_vsync = Some(self.settings.vsync);
        }
        if self.settings.show_fps {
            let dt = ctx.input(|i| i.unstable_dt).max(1e-5);
            self.fps = if self.fps == 0.0 { 1.0 / dt } else { self.fps * 0.95 + 0.05 / dt };
            ctx.request_repaint();
        }

        self.shortcuts(&ctx);
        self.update_animation(&ctx);

        egui::Panel::top("title_bar")
            .exact_size(shell::TITLE_HEIGHT)
            .frame(Frame::new().fill(theme::BG_APP).inner_margin(Margin { left: 16, right: 8, top: 0, bottom: 0 }))
            .show(ui, |ui| self.title_bar(ui));
        egui::Panel::bottom("footer")
            .exact_size(shell::FOOTER_HEIGHT)
            .frame(Frame::new().fill(theme::BG_APP).inner_margin(Margin::symmetric(16, 0)))
            .show(ui, |ui| self.footer(ui));
        if self.anim.as_ref().is_some_and(|a| a.has_clips()) {
            egui::Panel::bottom("timeline")
                .exact_size(48.0)
                .frame(Frame::new().fill(theme::BG_APP).inner_margin(Margin::symmetric(8, 0)))
                .show(ui, |ui| self.timeline(ui));
        }
        egui::Panel::left("tool_rail")
            .exact_size(shell::RAIL_WIDTH)
            .resizable(false)
            .frame(Frame::new().fill(theme::BG_APP))
            .show(ui, |ui| self.tool_rail(ui));
        if self.settings.show_sidebar {
            egui::Panel::right("inspector")
                .resizable(true)
                .default_size(shell::INSPECTOR_WIDTH)
                .size_range(280.0..=480.0)
                .frame(Frame::new().fill(theme::PANEL))
                .show(ui, |ui| self.inspector(ui));
        }
        egui::CentralPanel::no_frame().show(ui, |ui| self.viewport(ui, frame));

        let (shading, xray) = (self.settings.shading, self.settings.xray());
        match self.pie.show(&ctx, shading, xray) {
            Some(PieChoice::Shading(mode)) => self.settings.shading = mode,
            Some(PieChoice::ToggleXray) => self.settings.toggle_xray(),
            None => {}
        }
        self.preferences(&ctx);
        self.welcome(&ctx);
        self.banner(&ctx);
        self.window_edges(&ctx);
        self.drive_capture(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if self.capture.is_some() {
            return;
        }
        eframe::set_value(storage, eframe::APP_KEY, &self.settings);
    }
}

// --- Overlay helpers -----------------------------------------------------------------------

/// The app icon as a texture, for the title bar and the empty state.
fn load_logo(ctx: &egui::Context) -> TextureHandle {
    let png = include_bytes!("../assets/brand/icon/icon-128.png");
    let rgba = image::load_from_memory_with_format(png, image::ImageFormat::Png).expect("bundled icon").to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let image = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
    ctx.load_texture("app_logo", image, TextureOptions::LINEAR)
}

/// Start of the loader's "Missing texture: NAME" warning in the current language.
fn missing_prefix() -> String {
    tr("Missing texture: {name}").split('{').next().unwrap_or_default().to_string()
}

/// Adaptive grid: a new decade of lines fades in as you zoom. Returns (cell, fade, plane axis).
fn grid_params(camera: &Camera) -> (f32, f32, u32) {
    let level = camera.view.distance.max(1e-6).log10() - GRID_LEVEL_OFFSET;
    let axis = match (camera.ortho, camera.axis_view) {
        (true, Some(AxisView::Front | AxisView::Back)) => 1,
        (true, Some(AxisView::Right | AxisView::Left)) => 0,
        _ => 2,
    };
    (10f32.powf(level.floor()), level - level.floor(), axis)
}

/// Key combination text with translatable key names ("Wheel", "Space").
fn key_names(keys: &str) -> String {
    tr(keys).replace("Wheel", tr("Wheel"))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Formats a length in meters (1 unit = 1 m, like Blender) with a readable unit.
fn fmt_len(m: f32) -> String {
    let a = m.abs();
    let (value, unit) = if a >= 1000.0 {
        (m / 1000.0, "km")
    } else if a >= 1.0 {
        (m, "m")
    } else if a >= 0.01 {
        (m * 100.0, "cm")
    } else if a >= 0.001 {
        (m * 1000.0, "mm")
    } else if a == 0.0 {
        return "0 m".into();
    } else {
        (m * 1e6, "µm")
    };
    let decimals = if value.abs() >= 100.0 { 0 } else if value.abs() >= 10.0 { 1 } else { 2 };
    let text = format!("{value:.decimals$}");
    let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text };
    format!("{} {unit}", crate::i18n::decimal(text))
}
