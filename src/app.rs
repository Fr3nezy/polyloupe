//! Application shell: window layout, input handling, file loading and viewport overlays.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Frame, Key, Layout, Margin, Modifiers,
    PointerButton, Pos2, Rect, RectAlign, RichText, Sense, Stroke, StrokeKind, TextureHandle,
    TextureOptions, Ui, UiBuilder, pos2, vec2,
};
use glam::{Mat4, Vec3};
use eframe::egui_wgpu;

mod compare;
mod shell;
mod transform;
mod turntable;
mod uv_pane;

use crate::anim::AnimPlayer;
use crate::camera::{AxisView, Camera};
use crate::cli::{CaptureOptions, LaunchOptions};
use crate::update;
use crate::i18n::{self, Language, tr, trf};
use crate::instance;
use shell::{InspectorTab, ToastAction, Tool};
use crate::navigation::{DragInput, Gesture, Navigation};
use crate::loader::{self, EnvImage};
use crate::render::{self, FrameInput, Renderer, environment};
use crate::qa;
use crate::snap;
use crate::scene::{Aabb, MapRef, Scene};
use crate::settings::{ColorMode, Environment, Settings, ShadingMode, TexturePass, Workspace};
use crate::ui::gizmo::{self, GizmoAction};
use crate::ui::pie::{PieChoice, PieMenu};
use crate::ui::popovers::{self, ChannelAction, PopoverAction, Thumbnails};
use crate::ui::sidebar::SidebarAction;
use crate::ui::timeline::{self, TimelineAction, TimelineView};
use crate::ui::widgets::{self};
use crate::ui::{icons, theme};

const APP_NAME: &str = "PolyLoupe";
/// Public repository: issues, discussions and releases.
const REPO_URL: &str = "https://github.com/Fr3nezy/polyloupe";
/// Picks which decade of grid lines is visible for a given camera distance.
const GRID_LEVEL_OFFSET: f32 = 1.05;

struct ObjectMeta {
    name: String,
    triangles: usize,
    vertices: usize,
    /// World-space bounds.
    bounds: Aabb,
    material: usize,
    /// Pivot (object origin) in world space, rest pose, and its local X, Y and Z directions.
    origin: Vec3,
    axes: [Vec3; 3],
    /// The same before the Move tool's transform.
    rest_bounds: Aabb,
    rest_origin: Vec3,
    rest_axes: [Vec3; 3],
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
    units: crate::scene::Units,
    /// Mesh analysis, filled in by a background thread shortly after loading.
    qa: Option<qa::Report>,
}

/// A cut through the model along a world axis.
#[derive(Clone, Copy)]
struct Section {
    /// 0 = X, 1 = Y, 2 = Z.
    axis: usize,
    /// Position across the scene bounds, 0..1.
    t: f32,
    /// Keep the other side.
    flip: bool,
}

impl Default for Section {
    fn default() -> Self {
        Self { axis: 0, t: 0.5, flip: false }
    }
}

/// Mesh analysis totals, marker instances and snapping data, per mesh.
struct QaResult {
    report: qa::Report,
    markers: Vec<Vec<render::MarkerInstance>>,
    snap: Vec<snap::SnapMesh>,
    uv: crate::uv::UvData,
}

/// What a GPU pick is for.
#[derive(Clone, Copy)]
enum PickPurpose {
    Select { extend: bool },
    /// Place a measurement point; `free` (Ctrl) skips vertex snapping.
    MeasureClick { free: bool },
    /// Snap indicator and rubber band under the cursor.
    MeasureHover { free: bool },
    /// Move tool: lay the clicked face on the bed.
    LayFace,
    /// Move tool: the face under the cursor while choosing one to lay on the bed.
    LayHover,
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

/// How long the hidden window waits for the model read at startup before it shows up anyway,
/// with the model still loading.
const PRELOAD_WAIT: Duration = Duration::from_millis(1000);

/// Set when the saved window was maximized: `main` creates it un-maximized, because Windows shows
/// a window as soon as it is maximized, long before its first frame is painted. The first frame
/// maximizes it again as eframe shows it.
pub static RESTORE_MAXIMIZED: AtomicBool = AtomicBool::new(false);

/// The model given on the command line, read in the background from the start of `main`, so
/// parsing overlaps the window and GPU setup.
pub struct Preload {
    path: PathBuf,
    rx: Receiver<Result<Scene, String>>,
    started: Instant,
    /// Filled once the app exists, so a load that outlasts [`PRELOAD_WAIT`] can wake the UI.
    repaint: Arc<OnceLock<egui::Context>>,
}

impl Preload {
    /// `started` is when the app was launched: the "opened in" time counts from there.
    pub fn start(path: PathBuf, started: Instant) -> Self {
        let (tx, rx) = channel();
        let repaint = Arc::new(OnceLock::<egui::Context>::new());
        let (thread_path, thread_repaint) = (path.clone(), repaint.clone());
        std::thread::spawn(move || {
            let t = Instant::now();
            let result = loader::load(&thread_path);
            log::info!("startup: model read in {} ms", t.elapsed().as_millis());
            let _ = tx.send(result);
            if let Some(ctx) = thread_repaint.get() {
                ctx.request_repaint();
            }
        });
        Self { path, rx, started, repaint }
    }
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
    /// Manufacturing or 3D Art tools for the model on screen (see `Workspace`).
    workspace: Workspace,
    /// Where the viewport toolbar was last drawn: the HUD chips, gizmo and channel strip move
    /// below it when the viewport is too narrow for them to share the top edge.
    toolbar_rect: Rect,
    camera: Camera,
    renderer: Option<Renderer>,
    info: Option<SceneInfo>,
    loading: Option<Loading>,
    /// Mesh analysis of the loaded scene: totals plus marker instances per mesh.
    qa_rx: Option<Receiver<QaResult>>,
    /// Answer of the daily "is there a newer release" check.
    update_rx: Option<Receiver<Option<String>>>,
    /// Vertices per object for the Measure tool's snapping (arrives with the analysis).
    snap: Vec<snap::SnapMesh>,
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
    /// Point under the cursor and whether it snapped to a vertex.
    measure_hover: Option<(Vec3, bool)>,
    measure_hover_px: Option<[u32; 2]>,
    /// Cross-section, when on.
    section: Option<Section>,
    /// Move tool: per-object transform on top of the file's placement, its undo history, the
    /// gizmo drag in progress, and whether the next click lays a face on the bed.
    user_transforms: Vec<Mat4>,
    transform_undo: Vec<Vec<Mat4>>,
    transform_drag: Option<transform::Drag>,
    lay_face_armed: bool,
    lay_hover: Option<(Vec3, Vec3)>,
    lay_hover_px: Option<[u32; 2]>,
    /// Second model for the A/B comparison, and one being loaded.
    compare: Option<compare::Compare>,
    compare_loading: Option<Loading>,
    /// Last environment loaded, so a comparison model can light with it too.
    env_image: Option<EnvImage>,
    /// UV layouts of model A (arrive with the mesh analysis) and the UV pane's state.
    uv: Option<std::sync::Arc<crate::uv::UvData>>,
    uv_view: uv_pane::UvView,
    /// Viewport before the UV pane takes its part.
    viewport_full_rect: Rect,
    env_loaded: Option<Environment>,
    env_loading: Option<(Environment, Receiver<Result<EnvImage, String>>)>,
    capture: Option<CaptureState>,
    show_prefs: bool,
    /// Turntable export window, and the encoding running in the background.
    show_turntable: bool,
    pub(super) turntable_job: Option<Receiver<Result<PathBuf, String>>>,
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
    /// Startup reveal of a window restored maximized (see [`RESTORE_MAXIMIZED`]).
    reveal: Reveal,
}

/// A window restored maximized stays cloaked (see `cloak.rs`) until a frame at its maximized
/// size has been painted.
#[derive(Clone, Copy, PartialEq)]
enum Reveal {
    Done,
    /// Cloaked; `painted` once a frame was drawn while maximized, `frames` counts as a safety net.
    Cloaked { painted: bool, frames: u32 },
}

/// Drives `--capture`: waits for the scene and view to settle, screenshots, saves, quits.
struct CaptureState {
    opts: CaptureOptions,
    frames: u32,
    requested: bool,
}

impl ViewerApp {
    pub fn new(cc: &eframe::CreationContext<'_>, launch: LaunchOptions, preload: Option<Preload>) -> Self {
        crate::startup_mark("window and GPU ready");
        theme::apply(&cc.egui_ctx);
        crate::startup_mark("theme and fonts");
        let mut settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, eframe::APP_KEY))
            .unwrap_or_default();
        settings.ensure_lights();
        // Every session starts from the default environment; picking another is a per-session look.
        settings.environment = settings.default_environment.clone();
        if let Some(c) = &launch.capture {
            c.apply(&mut settings);
        }
        let renderer = cc.wgpu_render_state.as_ref().map(|rs| {
            let info = rs.adapter.get_info();
            log::info!("GPU: {} ({:?}, {:?})", info.name, info.backend, info.device_type);
            let r = Renderer::new(&rs.device, &rs.queue);
            crate::startup_mark("renderer");
            r
        });
        let mut app = Self {
            settings,
            toolbar_rect: Rect::NOTHING,
            camera: Camera::default(),
            renderer,
            info: None,
            loading: None,
            qa_rx: None,
            update_rx: None,
            snap: Vec::new(),
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
            section: None,
            workspace: Workspace::Art,
            user_transforms: Vec::new(),
            transform_undo: Vec::new(),
            transform_drag: None,
            lay_face_armed: false,
            lay_hover: None,
            lay_hover_px: None,
            compare: None,
            compare_loading: None,
            env_image: None,
            uv: None,
            uv_view: uv_pane::UvView::default(),
            viewport_full_rect: Rect::NOTHING,
            env_loaded: None,
            env_loading: None,
            capture: launch.capture.map(|opts| CaptureState { opts, frames: 0, requested: false }),
            show_prefs: false,
            show_turntable: false,
            turntable_job: None,
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
            reveal: Reveal::Done,
        };
        app.workspace = app.settings.workspace;
        app.start_update_check();
        if RESTORE_MAXIMIZED.load(Ordering::Relaxed) && crate::cloak::set_cloaked(cc, true) {
            app.reveal = Reveal::Cloaked { painted: false, frames: 0 };
        }
        if let Some(preload) = preload {
            app.finish_preload(&cc.egui_ctx, preload);
        }
        app
    }

    /// Waits briefly for the model read at startup, so the first frame, which is when the window
    /// appears, already shows it. A slower file keeps loading in the background as usual.
    fn finish_preload(&mut self, ctx: &egui::Context, preload: Preload) {
        let _ = preload.repaint.set(ctx.clone());
        match preload.rx.recv_timeout(PRELOAD_WAIT) {
            Ok(result) => self.finish_loading(ctx, &preload.path, preload.started, result),
            Err(RecvTimeoutError::Timeout) => {
                self.loading = Some(Loading { path: preload.path, rx: preload.rx, started: preload.started });
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.finish_loading(ctx, &preload.path, preload.started, Err(tr("the loader stopped").into()));
            }
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
                "{ext} isn't supported yet. Supported: glTF, GLB, FBX, OBJ, STL, PLY, 3MF, DAE, STEP, and .hdr / .exr environments.",
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
            .add_filter("PLY", &["ply"])
            .add_filter("3MF", &["3mf"])
            .add_filter("COLLADA", &["dae"])
            .add_filter("STEP", &["step", "stp"])
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

    pub(super) fn export_image(&mut self, ctx: &egui::Context) {
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

    pub(super) fn export_resolution_px(&self) -> [u32; 2] {
        self.settings.render_resolution.dimensions(self.viewport_px, self.settings.render_custom_w, self.settings.render_custom_h)
    }

    /// Saves the current view as a PNG, with supersampling (SSAA) and chosen resolution.
    fn export_to(&mut self, path: &Path) -> Result<(), String> {
        let [target_w, target_h] = self.export_resolution_px();
        let ssaa = self.settings.render_ssaa.clamp(1, 4);

        // Render at supersampled resolution (up to 8192 px max texture dimension).
        const MAX_SIDE: u32 = 8192;
        let mut render_w = target_w * ssaa;
        let mut render_h = target_h * ssaa;
        if render_w > MAX_SIDE || render_h > MAX_SIDE {
            let k = MAX_SIDE as f32 / (render_w.max(render_h) as f32);
            render_w = (render_w as f32 * k).round() as u32;
            render_h = (render_h as f32 * k).round() as u32;
        }

        let transparent = self.settings.export_transparent || self.settings.transparent_background;
        match self.render_offscreen([render_w, render_h], transparent) {
            Some(([w, h], pixels)) => {
                if w != target_w || h != target_h {
                    let img = image::RgbaImage::from_raw(w, h, pixels)
                        .ok_or_else(|| tr("Couldn't render the image").to_string())?;
                    let downsampled = image::imageops::resize(&img, target_w, target_h, image::imageops::FilterType::Lanczos3);
                    downsampled.save(path)
                        .map_err(|e| trf("Couldn't save the image: {error}", &[("error", &e)]))
                } else {
                    image::save_buffer(path, &pixels, w, h, image::ColorType::Rgba8)
                        .map_err(|e| trf("Couldn't save the image: {error}", &[("error", &e)]))
                }
            }
            None => Err(tr("Couldn't render the image").to_string()),
        }
    }

    /// Renders the current view off screen, as an exported image looks: overlays per the
    /// overlay switch, grid only if the export asks for it, no selection highlight.
    fn render_offscreen(&mut self, size: [u32; 2], transparent: bool) -> Option<([u32; 2], Vec<u8>)> {
        let section = self.section_plane();
        let normal_length = self.normal_length();
        let print_scale = self.print_scale();
        let scene = self.scene_frame();
        let renderer = self.renderer.as_mut()?;
        let mut settings = self.settings.clone();
        settings.show_grid = settings.show_overlays && settings.export_grid;
        settings.show_wire_overlay &= settings.show_overlays;
        settings.show_outline &= settings.show_overlays;
        settings.show_non_manifold &= settings.show_overlays;
        settings.show_open_edges &= settings.show_overlays;
        settings.show_overlapping &= settings.show_overlays;
        settings.show_normals &= settings.show_overlays;
        settings.show_face_orientation &= settings.show_overlays;
        if transparent {
            settings.env_background = false;
            settings.studio_backdrop = false;
            settings.transparent_background = false;
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
            transparent,
            section,
            normal_length,
            print_scale,
            scene,
        };
        renderer.render(None, size, &input);
        renderer.read_pixels()
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
                let before = s.navigation;
                egui::ComboBox::from_label(tr("Style"))
                    .selected_text(s.navigation.label())
                    .show_ui(ui, |ui| {
                        for nav in Navigation::ALL {
                            ui.selectable_value(&mut s.navigation, nav, nav.label());
                        }
                    });
                if s.navigation != before {
                    if let Some(up) = s.navigation.suggested_up() {
                        s.up_axis = up;
                    }
                }
                ui.horizontal(|ui| {
                    ui.label(tr("Up axis"));
                    widgets::segmented(ui, &mut s.up_axis, &[(crate::axes::UpAxis::Z, "Z up"), (crate::axes::UpAxis::Y, "Y up")]);
                });
                ui.label(
                    RichText::new(tr("Z: Blender, 3ds Max, Unreal. Y: Maya, Unity, Houdini, ZBrush, Substance. Only names, colors and numbers change; the model looks the same."))
                        .size(11.0)
                        .color(theme::TEXT_DIM),
                );
                ui.add_space(6.0);
                widgets::section(ui, "Interface");
                egui::ComboBox::from_label(tr("Language"))
                    .selected_text(s.language.label())
                    .show_ui(ui, |ui| {
                        for lang in Language::ALL {
                            ui.selectable_value(&mut s.language, lang, lang.label());
                        }
                    });
                ui.checkbox(&mut s.auto_workspace, tr("Pick the workspace from the file type")).on_hover_text(tr(
                    "STL, 3MF, STEP and PLY open in Manufacturing, the other formats in 3D Art. Off: the workspace you chose last",
                ));
                ui.checkbox(&mut s.check_updates, tr("Check for updates")).on_hover_text(tr(
                    "Once a day, PolyLoupe asks GitHub whether a newer release exists. Nothing is downloaded or installed",
                ));
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
                    .on_hover_text(tr("Opens Windows Default apps, where PolyLoupe can open .glb, .gltf, .fbx, .obj and .stl"))
                    .clicked()
                {
                    let _ = std::process::Command::new("explorer.exe")
                        .arg("ms-settings:defaultapps?registeredAppMachine=PolyLoupe")
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
            ui.label(RichText::new(tr("Welcome to PolyLoupe")).size(17.0).color(theme::TEXT));
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
                    if let Some(up) = nav.suggested_up() {
                        self.settings.up_axis = up;
                    }
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
        self.finish_loading(ctx, &loading.path, loading.started, result);
    }

    /// Shows a freshly read model. `started` is when opening was asked for, so the "opened in"
    /// time covers reading, uploading and (at launch) setting up the window and the GPU.
    fn finish_loading(&mut self, ctx: &egui::Context, path: &Path, started: Instant, result: Result<Scene, String>) {
        let result = result.and_then(|s| self.renderer.as_ref().map_or(Ok(()), |r| r.check_fits(&s)).map(|_| s));
        let mut scene = match result {
            Ok(scene) => scene,
            Err(err) => {
                let text = trf("Couldn't open {name}: {error}", &[("name", &file_name(path)), ("error", &err)]);
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
        let file_name = file_name(path);
        if self.settings.auto_workspace {
            self.workspace = Workspace::for_path(path);
        }
        self.camera.reset(&scene.bounds);
        let info = scene_info(&scene, path, started.elapsed());
        let n = info.objects.len();
        self.selection.reset(n);
        self.measures.clear();
        self.cancel_measure();
        self.snap.clear();
        self.section = None;
        self.user_transforms = vec![Mat4::IDENTITY; n];
        self.transform_undo.clear();
        self.transform_drag = None;
        self.lay_face_armed = false;
        self.uv = None;
        self.uv_view.reset();
        self.visible = vec![true; n];
        self.pass_override = vec![None; n];
        // Manufacturing ignores the file's textures, so missing ones aren't news.
        let missing_count = if self.manufacturing() { 0 } else { info.missing.len() };
        self.info = Some(info);
        self.apply_workspace();
        self.settings.push_recent(&path.to_string_lossy());
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
        self.qa_rx = Some(spawn_analysis(scene, ctx));
    }

    /// Moves a measurement point onto the nearest vertex of the object under the cursor,
    /// when one is within the snap radius. Returns the point and whether it snapped.
    fn snap_point(&self, object: Option<usize>, hit: Vec3, px: [u32; 2], free: bool, radius: f32) -> (Vec3, bool) {
        let mesh = object.filter(|_| !free).and_then(|o| self.snap.get(o));
        let Some(mesh) = mesh else { return (hit, false) };
        let [w, h] = self.viewport_px;
        let aspect = w as f32 / h.max(1) as f32;
        let view_proj = self.camera.projection(aspect) * self.camera.view_matrix();
        let cursor = glam::Vec2::new(px[0] as f32 + 0.5, px[1] as f32 + 0.5);
        let size = glam::Vec2::new(w as f32, h.max(1) as f32);
        match mesh.nearest(hit, cursor, view_proj, size, radius) {
            Some(v) => {
                log::debug!("measure point snapped: {hit} -> {v}");
                (v, true)
            }
            None => (hit, false),
        }
    }

    pub(super) fn manufacturing(&self) -> bool {
        self.workspace == Workspace::Manufacturing
    }

    /// Switches workspace for the model on screen; the choice is also the one used when
    /// "pick from the file type" is off.
    pub(super) fn set_workspace(&mut self, workspace: Workspace) {
        self.workspace = workspace;
        self.settings.workspace = workspace;
        self.apply_workspace();
    }

    /// Lengths and tools for the current workspace. Manufacturing shows millimeters, and reads
    /// files without units (STL, STEP) as millimeters too; 3D Art keeps Blender's meters.
    fn apply_workspace(&mut self) {
        let undeclared = self.info.as_ref().is_some_and(|i| i.units == crate::scene::Units::Undeclared);
        let manufacturing = self.manufacturing();
        set_length_units(if manufacturing && undeclared { 0.001 } else { 1.0 }, manufacturing);
        if manufacturing {
            self.uv_view.open = false;
            self.pass_override.fill(None);
            // Texture and Attribute (vertex colors) are 3D Art channels.
            if matches!(self.settings.color, ColorMode::Texture | ColorMode::Attribute) {
                self.settings.color = ColorMode::Material;
            }
            if self.inspector_tab == InspectorTab::Materials {
                self.inspector_tab = InspectorTab::Info;
            }
        } else if self.tool.transforms() {
            self.tool = Tool::Select;
        }
    }

    /// Where the visible model is, for the shadows and the shadow floor.
    pub(super) fn scene_frame(&self) -> Option<render::SceneFrame> {
        let b = self.visible_bounds(false);
        b.is_valid().then(|| render::SceneFrame { center: b.center(), radius: (b.max - b.min).length() * 0.5, floor: b.min.z })
    }

    /// World units per millimeter for the print finish, 0 when it doesn't apply.
    pub(super) fn print_scale(&self) -> f32 {
        match self.info.as_ref().map(|i| i.units) {
            _ if !self.manufacturing() => 0.0,
            Some(crate::scene::Units::Undeclared) => 1.0,
            _ => 0.001,
        }
    }

    /// Bounds of the whole scene, the range the section plane moves across.
    fn scene_bounds(&self) -> Aabb {
        let mut b = Aabb::EMPTY;
        if let Some(info) = &self.info {
            for o in &info.objects {
                b.union(&o.bounds);
            }
        }
        b
    }

    /// Normal line length in world units: a fraction of the scene's size.
    fn normal_length(&self) -> f32 {
        let b = self.scene_bounds();
        if b.is_valid() { b.radius() * self.settings.normal_size } else { 0.0 }
    }

    /// The section as a plane (unit normal, offset) for the renderer.
    fn section_plane(&self) -> Option<[f32; 4]> {
        let s = self.section?;
        let b = self.scene_bounds();
        if !b.is_valid() {
            return None;
        }
        let at = b.min[s.axis] + (b.max[s.axis] - b.min[s.axis]) * s.t;
        let sign = if s.flip { -1.0 } else { 1.0 };
        let mut n = [0.0; 3];
        n[s.axis] = sign;
        Some([n[0], n[1], n[2], sign * at])
    }

    /// Screen positions of the scene's two ends along the section axis (through its center).
    fn section_axis_on_screen(&self, viewport: Rect) -> Option<(Pos2, Pos2)> {
        let s = self.section?;
        let b = self.scene_bounds();
        let (mut lo, mut hi) = (b.center(), b.center());
        lo[s.axis] = b.min[s.axis];
        hi[s.axis] = b.max[s.axis];
        let aspect = viewport.width() / viewport.height().max(1.0);
        let view_proj = self.camera.projection(aspect) * self.camera.view_matrix();
        let project = |p: Vec3| {
            let c = view_proj * p.extend(1.0);
            (c.w > 1e-6).then(|| {
                let n = c.truncate() / c.w;
                pos2(viewport.left() + (n.x * 0.5 + 0.5) * viewport.width(), viewport.top() + (0.5 - n.y * 0.5) * viewport.height())
            })
        };
        Some((project(lo)?, project(hi)?))
    }

    fn cancel_measure(&mut self) {
        self.measure_start = None;
        self.measure_hover = None;
        self.measure_hover_px = None;
    }

    fn poll_qa(&mut self) {
        let Some(rx) = &self.qa_rx else { return };
        let Ok(QaResult { report, markers: instances, snap, uv }) = rx.try_recv() else { return };
        self.qa_rx = None;
        self.snap = snap;
        // The user may have moved objects while the analysis ran.
        self.transforms_changed(true);
        self.uv = Some(std::sync::Arc::new(uv));
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
                        if let Some(c) = &self.compare {
                            c.renderer.set_environment(&image);
                        }
                        if let Environment::File(path) = &env {
                            self.thumbs.add_environment(ctx, path, &image);
                        }
                        self.env_image = Some(image);
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
            if p.playing {
                ctx.request_repaint();
            }
            self.sync_compare_animation();
        } else if p.playing {
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

    /// Once a day, in the background: is a newer release published?
    fn start_update_check(&mut self) {
        let today = update::today();
        if !self.settings.check_updates || self.capture.is_some() || self.settings.last_update_check == today {
            return;
        }
        self.settings.last_update_check = today;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(update::newer_release());
        });
        self.update_rx = Some(rx);
    }

    fn poll_update(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.update_rx else { return };
        match rx.try_recv() {
            Ok(Some(version)) => {
                self.update_rx = None;
                self.show_banner(ctx, trf("PolyLoupe {version} is available", &[("version", &version)]), ToastAction::OpenReleases);
            }
            Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => self.update_rx = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_secs(1)),
        }
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
        self.transform_shortcuts(ctx);
        let keys = [
            (Key::Q, Tool::Select),
            (Key::O, Tool::Orbit),
            (Key::G, Tool::Pan),
            (Key::M, Tool::Measure),
            (Key::W, Tool::Move),
            (Key::E, Tool::Rotate),
            (Key::R, Tool::Scale),
        ];
        for (key, tool) in keys {
            if pressed(Modifiers::NONE, key) && (!tool.transforms() || self.manufacturing()) {
                self.tool = tool;
            }
        }
        if self.settings.navigation.f_frames() && pressed(Modifiers::NONE, Key::F) {
            self.frame_selected();
        }

        if self.info.is_some() && pressed(Modifiers::COMMAND | Modifiers::SHIFT, Key::O) {
            self.compare_dialog(ctx);
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
        if self.info.is_some() && !self.manufacturing() && pressed(Modifiers::NONE, Key::U) {
            self.uv_view.open = !self.uv_view.open;
        }
        // Measure: Esc drops the point being placed, Delete (or Ctrl+Z) removes the last
        // measurement, Shift+Delete clears them all.
        if self.measure_start.is_some() && pressed(Modifiers::NONE, Key::Escape) {
            self.cancel_measure();
        }
        if !self.measures.is_empty() {
            if pressed(Modifiers::SHIFT, Key::Delete) || pressed(Modifiers::SHIFT, Key::Backspace) {
                self.measures.clear();
                self.cancel_measure();
            } else if pressed(Modifiers::NONE, Key::Delete) || pressed(Modifiers::NONE, Key::Backspace) {
                self.measures.pop();
            }
        }
        // Ctrl+Z: the last measurement with the Measure tool, otherwise the last move.
        if pressed(Modifiers::COMMAND, Key::Z) {
            if self.tool == Tool::Measure && !self.measures.is_empty() {
                self.measures.pop();
            } else if !self.undo_transform() {
                self.measures.pop();
            }
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

    /// `gizmo`: the pointer is on the Move tool's gizmo, so a click there doesn't select.
    fn navigate(&mut self, ui: &Ui, response: &egui::Response, viewport: Rect, gizmo: bool) {
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
            Tool::Measure | Tool::Section | Tool::Move | Tool::Rotate | Tool::Scale => None,
        };
        // Section tool: a left drag slides the plane along its axis, following the axis
        // direction on screen.
        if self.tool == Tool::Section && drag.left && !mods.any() {
            if let (Some(section), Some((a, b))) = (self.section, self.section_axis_on_screen(viewport)) {
                let axis = b - a;
                let d = response.drag_delta();
                let step = (d.x * axis.x + d.y * axis.y) / axis.length_sq().max(1.0);
                self.section = Some(Section { t: (section.t + step).clamp(0.0, 1.0), ..section });
            }
        }
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
        if response.clicked_by(PointerButton::Primary) && !mods.alt && self.info.is_some() && !gizmo {
            if let Some(pos) = response.interact_pointer_pos() {
                let purpose = if self.tool == Tool::Measure {
                    PickPurpose::MeasureClick { free: mods.ctrl }
                } else if self.tool.transforms() && self.lay_face_armed {
                    PickPurpose::LayFace
                } else {
                    PickPurpose::Select { extend: mods.shift }
                };
                self.pending_pick = Some((to_px(pos), purpose));
            }
        } else if self.tool.transforms() && self.lay_face_armed && self.info.is_some() && self.pending_pick.is_none() {
            // A --lay capture has no real pointer: keep its simulated one.
            let simulated = self.capture.as_ref().is_some_and(|c| c.opts.lay_hover.is_some());
            let hover = if simulated { self.lay_hover_px } else { response.hover_pos().map(to_px) };
            if hover != self.lay_hover_px {
                self.lay_hover_px = hover;
                match hover {
                    Some(p) => self.pending_pick = Some((p, PickPurpose::LayHover)),
                    None => self.lay_hover = None,
                }
            }
        } else if self.tool == Tool::Measure && self.info.is_some() && self.pending_pick.is_none() {
            // Follow the cursor with a pick only when it moves: each pick waits for the GPU.
            let hover = response.hover_pos().map(to_px);
            if hover != self.measure_hover_px {
                self.measure_hover_px = hover;
                match hover {
                    Some(p) => self.pending_pick = Some((p, PickPurpose::MeasureHover { free: mods.ctrl })),
                    None => self.measure_hover = None,
                }
            }
        }
        if (self.tool == Tool::Measure || (self.tool.transforms() && self.lay_face_armed)) && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    // --- Layout -----------------------------------------------------------------------------

    fn viewport(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let full = ui.max_rect();
        self.viewport_full_rect = full;
        let (rect, uv_rect) = self.uv_split(full);
        self.viewport_rect = rect;
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        let ctx = ui.ctx().clone();

        if self.info.is_none() && self.loading.is_none() {
            self.empty_state(ui, rect);
        } else {
            if !self.pie.is_open() {
                let gizmo = self.transform_input(ui, &response, self.compare_rect_a(rect));
                self.navigate(ui, &response, rect, gizmo);
            }
            let dt = ui.input(|i| i.stable_dt).min(0.05);
            if self.camera.update(dt) {
                ctx.request_repaint();
            }
            let rect_a = self.compare_rect_a(rect);
            self.render_viewport(ui, frame, rect_a);
            self.render_compare(ui, frame, rect);
            self.draw_measures(ui, rect_a);
            self.draw_section(ui, rect_a);
            self.draw_origins(ui, rect_a);
            self.draw_transform_gizmo(ui, rect_a);
            self.draw_render_framing_guide(ui, rect_a);
            self.draw_bounds_overlay(ui, rect_a);
            self.draw_lights_overlay(ui, rect_a);

            let overlays = self.settings.show_overlays;
            if overlays {
                self.hud(ui, rect);
            }
            let gizmo_shown = overlays && self.settings.show_gizmo;
            if self.loading.is_none() && !self.manufacturing() {
                self.channel_strip(&ctx, rect, gizmo_shown);
            }
            if gizmo_shown {
                let top = self.overlay_top(rect, rect.right() - 16.0 - gizmo::WIDTH, rect.right() - 16.0);
                let center = pos2(rect.right() - gizmo::WIDTH * 0.5 - 16.0, top + gizmo::WIDTH * 0.5);
                let ppp = ctx.pixels_per_point();
                for action in gizmo::show(ui, center, &self.camera, self.settings.up_axis) {
                    match action {
                        GizmoAction::Orbit(d) => self.camera.orbit(d.x * ppp, d.y * ppp),
                        GizmoAction::AxisView(v) => self.camera.set_axis_view(v),
                    }
                }
            }
            self.viewport_toolbar(&ctx, rect);
            self.section_bar(&ctx, rect);
            self.transform_bar(&ctx, rect);
            self.compare_controls(ui, rect);
            if let Some(loading) = &self.loading {
                shell::loading_overlay(ui, rect, &file_name(&loading.path));
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }

        if let Some(uv_rect) = uv_rect {
            self.uv_pane(ui, uv_rect);
        }
        let hovering_files = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if hovering_files {
            shell::drop_overlay(ui, rect, self.info.is_some());
        }
    }

    /// Settings as the renderer should see them: the overlays switch hides every overlay.
    fn effective_settings(&self) -> Settings {
        let mut effective = self.settings.clone();
        if !effective.show_overlays {
            effective.show_grid = false;
            effective.show_wire_overlay = false;
            effective.show_bounds_overlay = false;
            effective.show_outline = false;
            effective.show_non_manifold = false;
            effective.show_open_edges = false;
            effective.show_overlapping = false;
            effective.show_normals = false;
            effective.show_face_orientation = false;
        }
        effective
    }

    fn render_viewport(&mut self, ui: &mut Ui, frame: &mut eframe::Frame, rect: Rect) {
        let section = self.section_plane();
        let normal_length = self.normal_length();
        let effective = self.effective_settings();
        let print_scale = self.print_scale();
        let scene = self.scene_frame();
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
        renderer.set_user_transforms(&self.user_transforms);

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
            section,
            normal_length,
            print_scale,
            scene,
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
        let radius = snap::RADIUS_PX * ui.ctx().pixels_per_point();
        match (pick.map(|p| p.1), hit) {
            (Some(PickPurpose::Select { extend }), Some(hit)) => {
                self.selection.click(hit, extend);
                ui.ctx().request_repaint();
            }
            (Some(PickPurpose::MeasureClick { free }), _) => {
                let px = pick.map(|p| p.0).unwrap_or_default();
                if let Some((p, _)) = point.map(|p| self.snap_point(hit.flatten(), p, px, free, radius)) {
                    match self.measure_start.take() {
                        None => self.measure_start = Some(p),
                        Some(a) => self.measures.push([a, p]),
                    }
                    self.measure_hover = None;
                    self.measure_hover_px = None;
                }
                ui.ctx().request_repaint();
            }
            (Some(PickPurpose::MeasureHover { free }), _) => {
                let px = pick.map(|p| p.0).unwrap_or_default();
                self.measure_hover = point.map(|p| self.snap_point(hit.flatten(), p, px, free, radius));
                ui.ctx().request_repaint();
            }
            (Some(PickPurpose::LayFace), _) => {
                self.lay_on_picked_face(ui.ctx(), hit.flatten(), point);
                ui.ctx().request_repaint();
            }
            (Some(PickPurpose::LayHover), _) => {
                self.lay_hover_at(hit.flatten(), point);
                ui.ctx().request_repaint();
            }
            _ => {}
        }
    }

    fn drive_capture(&mut self, ctx: &egui::Context) {
        let mut pending_channel = None;
        let mut pending_compare = None;
        let mut pending_uv_material = None;
        let Some(cap) = &mut self.capture else { return };
        ctx.request_repaint();
        if self.loading.is_some() || self.env_loading.is_some() {
            return;
        }
        if cap.requested && cap.opts.turntable.is_some() {
            if self.turntable_job.is_none() {
                println!("turntable done");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        }
        // Hold the shot until the model, mesh analysis (and model B) are in.
        if cap.frames > 2 && (self.loading.is_some() || self.qa_rx.is_some() || self.compare_loading.is_some()) {
            return;
        }
        if cap.opts.compare_split {
            if let Some(c) = &mut self.compare {
                c.mode = compare::CompareMode::Split;
            }
        }
        cap.frames += 1;
        // Move tool steps run once the mesh analysis is in (frame 3 waits for it).
        let (frames, rotate, auto_orient, export_model, lay_hover) =
            (cap.frames, cap.opts.rotate, cap.opts.auto_orient, cap.opts.export_model.clone(), cap.opts.lay_hover);
        if let (4, Some(p)) = (frames, lay_hover) {
            self.tool = Tool::Move;
            self.lay_face_armed = true;
            self.lay_hover_px = Some(p);
            self.pending_pick = Some((p, PickPurpose::LayHover));
        }
        if frames == 3 {
            if let Some([x, y, z]) = rotate {
                let r = Mat4::from_euler(glam::EulerRot::XYZ, x.to_radians(), y.to_radians(), z.to_radians());
                for i in self.transform_targets() {
                    self.user_transforms[i] = r * self.user_transforms[i];
                }
                self.transforms_changed(true);
            }
        }
        if frames == 4 && auto_orient {
            self.auto_orient(ctx);
        }
        if let (5, Some(out)) = (frames, export_model) {
            match self.export_model_to(&out) {
                Ok(()) => println!("exported {}", out.display()),
                Err(e) => eprintln!("export failed: {e}"),
            }
        }
        let Some(cap) = &mut self.capture else { return };
        if let (4, Some([_, b])) = (cap.frames, cap.opts.measure) {
            self.pending_pick = Some((b, PickPurpose::MeasureClick { free: false }));
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
                Some("render") => {
                    self.settings.show_sidebar = true;
                    self.inspector_tab = InspectorTab::Render;
                }
                _ => {}
            }
            if cap.opts.pie {
                self.pie.open(ctx.content_rect().center(), ctx.input(|i| i.time));
            }
            match cap.opts.tool.as_deref() {
                Some("move") => self.tool = Tool::Move,
                Some("rotate") => self.tool = Tool::Rotate,
                Some("scale") => self.tool = Tool::Scale,
                _ => {}
            }
            if cap.opts.uv {
                self.uv_view.open = true;
            }
            pending_uv_material = cap.opts.uv_material;
            if let Some(path) = cap.opts.compare.clone() {
                pending_compare = Some(path);
            }
            if let Some((axis, t, flip)) = cap.opts.section {
                self.section = Some(Section { axis, t, flip });
            }
            if let Some([a, _]) = cap.opts.measure {
                self.tool = Tool::Measure;
                self.pending_pick = Some((a, PickPurpose::MeasureClick { free: false }));
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
            if let Some(out) = cap.opts.turntable.clone() {
                self.settings.turntable_mp4 = out.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
                self.export_turntable_to(out, ctx);
                return;
            }
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
        if let Some(m) = pending_uv_material {
            self.uv_show_material(m);
        }
        if let Some(path) = pending_compare {
            self.open_compare(path, ctx);
        }
    }
}

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        if RESTORE_MAXIMIZED.swap(false, Ordering::Relaxed) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        if let Reveal::Cloaked { painted, frames } = self.reveal {
            let maximized = ctx.input(|i| i.viewport().maximized == Some(true));
            // The previous frame was painted maximized: show it. Give up waiting after a few
            // frames rather than ever leaving the window invisible.
            if painted || frames > 10 {
                crate::cloak::set_cloaked(&*frame, false);
                self.reveal = Reveal::Done;
            } else {
                self.reveal = Reveal::Cloaked { painted: maximized, frames: frames + 1 };
                ctx.request_repaint();
            }
        }
        self.sync_preferences(&ctx);
        if let Some(path) = self.pending_open.take() {
            self.open(path, &ctx);
        }
        self.poll_loading(&ctx);
        self.poll_qa();
        self.poll_update(&ctx);
        self.poll_compare(&ctx, frame);
        self.poll_turntable(&ctx);
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
            let pointer = ctx.input(|i| i.pointer.hover_pos());
            if self.info.is_some() && pointer.is_some_and(|p| p.x > self.viewport_rect.center().x) {
                self.open_compare(path, &ctx);
            } else {
                self.open(path, &ctx);
            }
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
                .size_range(340.0..=520.0)
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
        self.turntable_window(&ctx);
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
/// What the Info tab and the rest of the UI keep of a loaded scene.
fn scene_info(scene: &Scene, path: &Path, load_time: Duration) -> SceneInfo {
    SceneInfo {
        path: path.to_path_buf(),
        missing: scene
            .images
            .iter()
            .filter_map(|i| i.name.strip_suffix(loader::MISSING_SUFFIX).map(str::to_string))
            .collect(),
        file_name: file_name(path),
        materials: scene.materials.iter().map(|m| MaterialMeta { name: m.name.clone(), maps: m.maps() }).collect(),
        images: scene.images.iter().map(|i| (i.name.clone(), i.width, i.height)).collect(),
        objects: scene
            .meshes
            .iter()
            .map(|m| {
                let bounds = m.bounds.transformed(&m.transform);
                let origin = m.transform.w_axis.truncate();
                // Undo the file's up-axis conversion so an unrotated object shows X, Y, Z like
                // the navigation gizmo (and Blender), not its Y-up file axes.
                let t = m.transform * Mat4::from_quat(scene.axis_conversion.inverse());
                let axes = [t.x_axis, t.y_axis, t.z_axis].map(|a| a.truncate().normalize_or_zero());
                ObjectMeta {
                    name: m.name.clone(),
                    triangles: m.triangle_count(),
                    vertices: m.positions.len(),
                    bounds,
                    material: m.material.min(scene.materials.len() - 1),
                    origin,
                    axes,
                    rest_bounds: bounds,
                    rest_origin: origin,
                    rest_axes: axes,
                }
            })
            .collect(),
        vertices: scene.source_vertex_count,
        triangles: scene.triangle_count(),
        load_time,
        units: scene.units,
        qa: None,
    }
}

/// Runs the mesh analysis on a worker thread; the scene's CPU copy is dropped there.
fn spawn_analysis(scene: Scene, ctx: &egui::Context) -> Receiver<QaResult> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let (report, marks) = qa::analyze(&scene.meshes);
        log::info!("mesh analysis in {:?}: {report:?}", started.elapsed());
        let markers = scene.meshes.iter().zip(&marks).map(|(m, k)| render::marker_instances(m, k)).collect();
        let uv = crate::uv::extract(&scene.meshes, &scene.materials, &scene.images);
        let snap = scene.meshes.into_iter().map(|m| snap::SnapMesh::new(m.transform, m.positions, m.indices)).collect();
        if tx.send(QaResult { report, markers, snap, uv }).is_ok() {
            ctx.request_repaint();
        }
    });
    rx
}

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

/// Meters per world unit (bits of an f32) and whether lengths always show in millimeters:
/// set per model by `apply_workspace`.
static METERS_PER_UNIT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000);
static PREFER_MM: AtomicBool = AtomicBool::new(false);

fn set_length_units(meters_per_unit: f32, millimeters: bool) {
    METERS_PER_UNIT.store(meters_per_unit.to_bits(), Ordering::Relaxed);
    PREFER_MM.store(millimeters, Ordering::Relaxed);
}

fn meters_per_unit() -> f32 {
    f32::from_bits(METERS_PER_UNIT.load(Ordering::Relaxed))
}

/// Formats a length in world units (1 unit = 1 m like Blender, or 1 mm for unitless files in
/// the Manufacturing workspace) with a readable unit; millimeters in Manufacturing.
fn fmt_len(units: f32) -> String {
    let m = units * meters_per_unit();
    if PREFER_MM.load(Ordering::Relaxed) {
        let mm = m * 1000.0;
        let decimals = if mm.abs() >= 100.0 { 1 } else { 2 };
        let text = format!("{mm:.decimals$}");
        let text = if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text };
        let text = if text == "-0" { "0".to_string() } else { text };
        return format!("{text} mm");
    }
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
