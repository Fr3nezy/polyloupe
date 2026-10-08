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
pub enum WireColorMode {
    Theme,
    Random,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomLight {
    pub name: String,
    pub enabled: bool,
    pub follow_env: bool,
    pub yaw: f32,
    pub pitch: f32,
    pub strength: f32,
    pub color: [u8; 3],
}

impl Default for CustomLight {
    fn default() -> Self {
        Self {
            name: "Light".to_string(),
            enabled: true,
            follow_env: false,
            yaw: 45.0,
            pitch: 55.0,
            strength: 1.0,
            color: [255, 255, 255],
        }
    }
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

/// What a part is made of (Manufacturing workspace). Rendered mode shows its sheen, layer lines
/// and texture, procedural or projected in world space so no UVs are needed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartMaterial {
    Pla,
    PlaSilk,
    Petg,
    Abs,
    Resin,
    Nylon,
    Metal,
}

/// The families the material picker shows first; FDM plastic and metal have a second row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialGroup {
    Fdm,
    Resin,
    Nylon,
    Metal,
}

impl MaterialGroup {
    pub const ALL: [MaterialGroup; 4] = [MaterialGroup::Fdm, MaterialGroup::Resin, MaterialGroup::Nylon, MaterialGroup::Metal];

    pub fn label(self) -> &'static str {
        match self {
            MaterialGroup::Fdm => "FDM plastic",
            MaterialGroup::Resin => "Resin",
            MaterialGroup::Nylon => "Nylon SLS",
            MaterialGroup::Metal => "Metal",
        }
    }

    /// The material a click on the family picks.
    pub fn first(self) -> PartMaterial {
        match self {
            MaterialGroup::Fdm => PartMaterial::Pla,
            MaterialGroup::Resin => PartMaterial::Resin,
            MaterialGroup::Nylon => PartMaterial::Nylon,
            MaterialGroup::Metal => PartMaterial::Metal,
        }
    }
}

impl PartMaterial {
    pub const FDM: [PartMaterial; 4] = [PartMaterial::Pla, PartMaterial::PlaSilk, PartMaterial::Petg, PartMaterial::Abs];

    pub fn group(self) -> MaterialGroup {
        match self {
            PartMaterial::Pla | PartMaterial::PlaSilk | PartMaterial::Petg | PartMaterial::Abs => MaterialGroup::Fdm,
            PartMaterial::Resin => MaterialGroup::Resin,
            PartMaterial::Nylon => MaterialGroup::Nylon,
            PartMaterial::Metal => MaterialGroup::Metal,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PartMaterial::Pla => "PLA",
            PartMaterial::PlaSilk => "PLA Silk",
            PartMaterial::Petg => "PETG",
            PartMaterial::Abs => "ABS",
            PartMaterial::Resin => "Resin",
            PartMaterial::Nylon => "Nylon SLS",
            PartMaterial::Metal => "Metal",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            PartMaterial::Pla => "Matte, visible layer lines",
            PartMaterial::PlaSilk => "Satin metallic sheen, layer lines",
            PartMaterial::Petg => "Glossy, visible layer lines",
            PartMaterial::Abs => "Satin, visible layer lines",
            PartMaterial::Resin => "SLA/MSLA, smooth with fine layers",
            PartMaterial::Nylon => "Powder bed, grainy and matte",
            PartMaterial::Metal => "Machined or sintered metal",
        }
    }

    /// Typical layer height in millimeters; 0 for parts without visible layers.
    pub fn layer_height(self) -> f32 {
        match self.group() {
            MaterialGroup::Fdm => 0.2,
            MaterialGroup::Resin => 0.05,
            MaterialGroup::Nylon | MaterialGroup::Metal => 0.0,
        }
    }

    /// Index the mesh shader switches on (1..).
    pub fn shader_id(self) -> u32 {
        match self {
            PartMaterial::Pla => 1,
            PartMaterial::PlaSilk => 2,
            PartMaterial::Petg => 3,
            PartMaterial::Abs => 4,
            PartMaterial::Resin => 5,
            PartMaterial::Nylon => 6,
            PartMaterial::Metal => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetalFinish {
    Polished,
    Satin,
    Brushed,
    Blasted,
}

impl MetalFinish {
    pub const ALL: [MetalFinish; 4] = [MetalFinish::Polished, MetalFinish::Satin, MetalFinish::Brushed, MetalFinish::Blasted];

    pub fn label(self) -> &'static str {
        match self {
            MetalFinish::Polished => "Polished",
            MetalFinish::Satin => "Satin",
            MetalFinish::Brushed => "Brushed",
            MetalFinish::Blasted => "Blasted",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderResolution {
    Viewport,
    Viewport2x,
    Fhd1080p,
    Qhd1440p,
    Uhd4k,
    Square1k,
    Square2k,
    Portrait,
    Custom,
}

impl RenderResolution {
    pub const ALL: [RenderResolution; 9] = [
        RenderResolution::Viewport,
        RenderResolution::Viewport2x,
        RenderResolution::Fhd1080p,
        RenderResolution::Qhd1440p,
        RenderResolution::Uhd4k,
        RenderResolution::Square1k,
        RenderResolution::Square2k,
        RenderResolution::Portrait,
        RenderResolution::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RenderResolution::Viewport => "Viewport (1×)",
            RenderResolution::Viewport2x => "Viewport (2×)",
            RenderResolution::Fhd1080p => "1080p FHD (16:9)",
            RenderResolution::Qhd1440p => "1440p 2K (16:9)",
            RenderResolution::Uhd4k => "2160p 4K (16:9)",
            RenderResolution::Square1k => "1080p Square (1:1)",
            RenderResolution::Square2k => "2048p Square (1:1)",
            RenderResolution::Portrait => "Portrait 4:5 (1080×1350)",
            RenderResolution::Custom => "Custom",
        }
    }

    pub fn dimensions(self, viewport_px: [u32; 2], custom_w: u32, custom_h: u32) -> [u32; 2] {
        match self {
            RenderResolution::Viewport => [viewport_px[0].max(1), viewport_px[1].max(1)],
            RenderResolution::Viewport2x => [viewport_px[0].max(1) * 2, viewport_px[1].max(1) * 2],
            RenderResolution::Fhd1080p => [1920, 1080],
            RenderResolution::Qhd1440p => [2560, 1440],
            RenderResolution::Uhd4k => [3840, 2160],
            RenderResolution::Square1k => [1080, 1080],
            RenderResolution::Square2k => [2048, 2048],
            RenderResolution::Portrait => [1080, 1350],
            RenderResolution::Custom => [custom_w.clamp(64, 8192), custom_h.clamp(64, 8192)],
        }
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
    /// Ask GitHub once a day whether a newer release exists, and say so in a banner.
    pub check_updates: bool,
    /// Day (since the Unix epoch) of the last check.
    pub last_update_check: u64,
    /// Which axis the interface calls up (display only, see `axes`).
    pub up_axis: crate::axes::UpAxis,
    /// Master switch, like Blender's overlays toggle.
    pub show_overlays: bool,
    pub show_grid: bool,
    pub show_axes: bool,
    pub show_bounds_overlay: bool,
    pub show_wire_overlay: bool,
    pub wire_color_mode: WireColorMode,
    pub wire_color: [u8; 3],
    pub wire_opacity: f32,
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
    /// Manufacturing: every part in one color (`plastic_color`, sRGB) and `material`, or the
    /// file's own materials when `file_materials` is on. 3D Art always uses the file's own.
    pub file_materials: bool,
    pub plastic_color: [u8; 3],
    pub material: PartMaterial,
    pub metal_finish: MetalFinish,
    /// FDM and resin layer lines (Rendered mode), and their height in millimeters.
    pub layer_lines: bool,
    pub layer_height: f32,
    /// Surface wear over any material, 0..1: fine grain and dust, and scratches. `grain_size`
    /// scales both patterns (1 = default size).
    pub grain: f32,
    pub scratches: f32,
    pub grain_size: f32,
    /// Rendered mode: a key light taken from the environment's brightest spot, casting shadows
    /// on the model, and a floor under it that catches its shadow and contact shading.
    pub shadows: bool,
    /// Rendered mode: a light studio backdrop (a soft gradient, no grid) instead of the
    /// viewport's dark gray. `env_background` wins over it.
    pub studio_backdrop: bool,
    /// Rendered mode: transparent background with a viewport checkerboard and alpha export.
    pub transparent_background: bool,
    pub floor_shadow: bool,
    pub light_strength: f32,
    /// 0 = crisp, 1 = very soft.
    pub shadow_softness: f32,
    /// Key light: follow environment brightest spot or custom angles.
    pub light_follow_env: bool,
    pub light_yaw: f32,
    pub light_pitch: f32,
    pub light_color: [u8; 3],

    /// Secondary / Fill light.
    pub light2_enabled: bool,
    pub light2_yaw: f32,
    pub light2_pitch: f32,
    pub light2_strength: f32,
    pub light2_color: [u8; 3],

    /// Multi-light list and viewport gizmo toggle.
    pub lights: Vec<CustomLight>,
    pub show_light_gizmos: bool,

    /// Dedicated Render tab resolution and quality options.
    pub render_resolution: RenderResolution,
    pub render_custom_w: u32,
    pub render_custom_h: u32,
    pub render_ssaa: u32,
    pub render_framing_guide: bool,
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
            check_updates: true,
            last_update_check: 0,
            up_axis: crate::axes::UpAxis::Z,
            show_overlays: true,
            show_grid: true,
            show_axes: true,
            show_bounds_overlay: true,
            show_wire_overlay: false,
            wire_color_mode: WireColorMode::Theme,
            wire_color: [40, 40, 40],
            wire_opacity: 0.75,
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
            layer_height: 0.2,
            file_materials: false,
            plastic_color: [200, 200, 196],
            material: PartMaterial::Pla,
            metal_finish: MetalFinish::Satin,
            layer_lines: true,
            grain: 0.3,
            scratches: 0.0,
            grain_size: 1.0,
            shadows: true,
            studio_backdrop: false,
            transparent_background: false,
            floor_shadow: true,
            light_strength: 1.0,
            shadow_softness: 0.4,
            light_follow_env: true,
            light_yaw: 45.0,
            light_pitch: 55.0,
            light_color: [255, 255, 255],
            light2_enabled: false,
            light2_yaw: 225.0,
            light2_pitch: 35.0,
            light2_strength: 0.6,
            light2_color: [215, 230, 255],
            lights: vec![
                CustomLight {
                    name: "Key Light".to_string(),
                    enabled: true,
                    follow_env: true,
                    yaw: 45.0,
                    pitch: 55.0,
                    strength: 1.0,
                    color: [255, 255, 255],
                },
                CustomLight {
                    name: "Fill Light".to_string(),
                    enabled: false,
                    follow_env: false,
                    yaw: 225.0,
                    pitch: 35.0,
                    strength: 0.6,
                    color: [215, 230, 255],
                },
            ],
            show_light_gizmos: true,
            render_resolution: RenderResolution::Viewport,
            render_custom_w: 1920,
            render_custom_h: 1080,
            render_ssaa: 2,
            render_framing_guide: true,
        }
    }
}

impl Settings {
    pub fn ensure_lights(&mut self) {
        if self.lights.is_empty() {
            self.lights = vec![
                CustomLight {
                    name: "Key Light".to_string(),
                    enabled: true,
                    follow_env: self.light_follow_env,
                    yaw: self.light_yaw,
                    pitch: self.light_pitch,
                    strength: self.light_strength,
                    color: self.light_color,
                },
                CustomLight {
                    name: "Fill Light".to_string(),
                    enabled: self.light2_enabled,
                    follow_env: false,
                    yaw: self.light2_yaw,
                    pitch: self.light2_pitch,
                    strength: self.light2_strength,
                    color: self.light2_color,
                },
            ];
        }
    }
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
