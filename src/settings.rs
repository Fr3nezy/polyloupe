//! User-facing viewport settings, persisted between sessions.

use serde::{Deserialize, Serialize};

use crate::i18n::Language;
use crate::navigation::Navigation;
use crate::render::environment::Preset;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShadingMode {
    Wireframe,
    Solid,
    Rendered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lighting {
    Studio,
    MatCap,
    Flat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMode {
    Material,
    Single,
    Random,
    /// Image textures, with a pass selector (Base Color, Roughness, ...).
    Texture,
    /// Vertex colors.
    Attribute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TexturePass {
    BaseColor,
    Roughness,
    Metallic,
    Normal,
    Occlusion,
    Emission,
    Alpha,
    UvChecker,
}

impl TexturePass {
    pub const ALL: [TexturePass; 8] = [
        TexturePass::BaseColor,
        TexturePass::Roughness,
        TexturePass::Metallic,
        TexturePass::Normal,
        TexturePass::Occlusion,
        TexturePass::Emission,
        TexturePass::Alpha,
        TexturePass::UvChecker,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TexturePass::BaseColor => "Base Color",
            TexturePass::Roughness => "Roughness",
            TexturePass::Metallic => "Metallic",
            TexturePass::Normal => "Normal",
            TexturePass::Occlusion => "AO",
            TexturePass::Emission => "Emission",
            TexturePass::Alpha => "Alpha",
            TexturePass::UvChecker => "UV Grid",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewTransform {
    Standard,
    AgX,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Environment {
    Preset(Preset),
    /// A user-provided equirectangular .hdr file.
    File(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub shading: ShadingMode,
    pub lighting: Lighting,
    pub color: ColorMode,
    /// sRGB, as shown in the color picker.
    pub single_color: [u8; 3],
    pub matcap: usize,
    /// X-ray is remembered per mode, like Blender (wireframe defaults to on).
    pub xray_solid: bool,
    pub xray_wire: bool,
    pub xray_alpha: f32,
    pub backface_culling: bool,
    pub exposure: f32,
    pub texture_pass: TexturePass,
    /// Solid mode object outline, like Blender's Options > Outline (an overlay here).
    pub show_outline: bool,

    pub environment: Environment,
    /// Environment the Rendered mode starts with.
    pub default_environment: Environment,
    /// The user's HDRIs (paths), shown next to the built-in ones.
    pub custom_environments: Vec<String>,
    /// Degrees around the world Z axis.
    pub env_rotation: f32,
    pub env_strength: f32,
    pub env_background: bool,
    pub env_blur: f32,
    pub view_transform: ViewTransform,

    pub show_sidebar: bool,
    /// Master switch, like Blender's overlays toggle.
    pub show_overlays: bool,
    pub show_grid: bool,
    pub show_axes: bool,
    pub show_wire_overlay: bool,
    /// Mesh analysis overlays (see `qa`).
    pub show_non_manifold: bool,
    pub show_open_edges: bool,
    pub show_overlapping: bool,
    /// Vertex normal lines and their length as a fraction of the scene size.
    pub show_normals: bool,
    pub normal_size: f32,
    /// Front faces blue, back faces red.
    pub show_face_orientation: bool,
    /// Object origins (pivots) as dots, like Blender.
    pub show_origins: bool,
    /// Triangle budget shown against the model in Info (0 = none).
    pub triangle_budget: usize,
    /// Turntable export options.
    pub turntable_mp4: bool,
    pub turntable_size: u32,
    pub turntable_seconds: f32,
    pub turntable_animate: bool,
    pub show_stats: bool,
    pub show_gizmo: bool,

    pub vsync: bool,
    pub show_fps: bool,

    pub language: Language,
    pub navigation: Navigation,
    /// The first-run welcome (navigation choice) was shown.
    pub onboarded: bool,
    /// Files opened from Explorer go to the window that's already open.
    pub single_window: bool,
    /// Image export: multiple of the viewport size.
    pub export_scale: u32,
    pub export_transparent: bool,
    pub export_grid: bool,

    pub recent_files: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shading: ShadingMode::Solid,
            lighting: Lighting::Studio,
            color: ColorMode::Material,
            single_color: [204, 204, 204],
            matcap: 0,
            xray_solid: false,
            xray_wire: true,
            xray_alpha: 0.5,
            backface_culling: false,
            exposure: 1.0,
            texture_pass: TexturePass::BaseColor,
            show_outline: true,
            environment: Environment::Preset(Preset::Forest),
            default_environment: Environment::Preset(Preset::Forest),
            custom_environments: Vec::new(),
            env_rotation: 0.0,
            env_strength: 1.0,
            env_background: false,
            env_blur: 0.4,
            view_transform: ViewTransform::AgX,
            show_sidebar: false,
            show_overlays: true,
            show_grid: true,
            show_axes: true,
            show_wire_overlay: false,
            show_non_manifold: false,
            show_open_edges: false,
            show_overlapping: false,
            show_normals: false,
            normal_size: 0.02,
            show_face_orientation: false,
            show_origins: false,
            triangle_budget: 0,
            turntable_mp4: false,
            turntable_size: 720,
            turntable_seconds: 4.0,
            turntable_animate: true,
            show_stats: true,
            show_gizmo: true,
            vsync: true,
            show_fps: false,
            language: Language::System,
            navigation: Navigation::Blender,
            onboarded: false,
            single_window: false,
            export_scale: 2,
            export_transparent: false,
            export_grid: false,
            recent_files: Vec::new(),
        }
    }
}

impl Settings {
    pub fn xray(&self) -> bool {
        match self.shading {
            ShadingMode::Wireframe => self.xray_wire,
            _ => self.xray_solid,
        }
    }

    pub fn toggle_xray(&mut self) {
        match self.shading {
            ShadingMode::Wireframe => self.xray_wire = !self.xray_wire,
            _ => self.xray_solid = !self.xray_solid,
        }
    }

    /// Adds a user HDRI to the library (most recent first) and selects it.
    pub fn add_environment(&mut self, path: &str) {
        self.custom_environments.retain(|p| p != path);
        self.custom_environments.insert(0, path.to_string());
        self.custom_environments.truncate(12);
        self.environment = Environment::File(path.to_string());
    }

    pub fn push_recent(&mut self, path: &str) {
        self.recent_files.retain(|p| p != path);
        self.recent_files.insert(0, path.to_string());
        self.recent_files.truncate(10);
    }
}
