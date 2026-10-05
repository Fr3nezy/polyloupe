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

/// Which tools the interface offers: for making physical parts (CAD, 3D printing) or for 3D
/// art (textures, UVs, animation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Workspace {
    Manufacturing,
    Art,
}

impl Workspace {
    /// The workspace a file type usually belongs to: print and CAD formats are manufacturing,
    /// the rest art.
    pub fn for_path(path: &std::path::Path) -> Workspace {
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("stl" | "3mf" | "step" | "stp" | "ply") => Workspace::Manufacturing,
            _ => Workspace::Art,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Workspace::Manufacturing => "Manufacturing",
            Workspace::Art => "3D Art",
        }
    }
}

/// Procedural look of a printed part (Manufacturing workspace): layer lines, grain, and the
/// material's sheen, projected in world space so no UVs are needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Finish {
    Off,
    Pla,
    Petg,
    SilkPla,
    Resin,
    Nylon,
    Metal,
}

impl Finish {
    pub const ALL: [Finish; 7] = [Finish::Off, Finish::Pla, Finish::Petg, Finish::SilkPla, Finish::Resin, Finish::Nylon, Finish::Metal];

    pub fn label(self) -> &'static str {
        match self {
            Finish::Off => "None",
            Finish::Pla => "PLA",
            Finish::Petg => "PETG",
            Finish::SilkPla => "Silk PLA",
            Finish::Resin => "Resin",
            Finish::Nylon => "Nylon SLS",
            Finish::Metal => "Metal SLM",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Finish::Off => "Smooth satin plastic, no layer lines",
            Finish::Pla => "FDM, matte, visible layer lines",
            Finish::Petg => "FDM, glossy, visible layer lines",
            Finish::SilkPla => "FDM, satin metallic sheen",
            Finish::Resin => "SLA/MSLA, smooth with fine layers",
            Finish::Nylon => "Powder bed, grainy and matte",
            Finish::Metal => "Laser-sintered metal, grainy",
        }
    }

    /// Typical layer height in millimeters.
    pub fn layer_height(self) -> f32 {
        match self {
            Finish::Off => 0.0,
            Finish::Pla | Finish::Petg | Finish::SilkPla => 0.2,
            Finish::Resin => 0.05,
            Finish::Nylon => 0.1,
            Finish::Metal => 0.04,
        }
    }

    /// Index the mesh shader switches on (0 = off).
    pub fn shader_id(self) -> u32 {
        Finish::ALL.iter().position(|f| *f == self).unwrap_or(0) as u32
    }
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
    /// Which axis the interface calls up (display only, see `axes`).
    pub up_axis: crate::axes::UpAxis,
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

    /// Pick the workspace from the file type; otherwise `workspace` always.
    pub auto_workspace: bool,
    pub workspace: Workspace,
    /// Print finish (Manufacturing workspace, Rendered mode) and its layer height in millimeters.
    pub finish: Finish,
    pub layer_height: f32,
    /// Manufacturing shows every part as plain plastic of `plastic_color` (sRGB; a new name, so an old filament choice no longer applies), ignoring the
    /// file's materials and textures, unless this is on. 3D Art always uses the file's own.
    pub file_materials: bool,
    pub plastic_color: [u8; 3],
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
            up_axis: crate::axes::UpAxis::Z,
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
            auto_workspace: true,
            workspace: Workspace::Art,
            finish: Finish::Pla,
            layer_height: 0.2,
            file_materials: false,
            plastic_color: [200, 200, 196],
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
