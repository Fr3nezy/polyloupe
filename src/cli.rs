//! Command line: `polyloupe [FILE] [--capture OUT.png] [state flags...]`.
//!
//! `--capture` renders the given state, saves a screenshot and exits. It drives visual
//! regression checks and README screenshots without touching the saved preferences.

use std::path::PathBuf;

use crate::camera::AxisView;
use crate::render::environment::Preset;
use crate::settings::{ColorMode, Environment, Lighting, Settings, ShadingMode, TexturePass, Workspace};

#[derive(Default)]
pub struct LaunchOptions {
    pub open: Option<PathBuf>,
    pub capture: Option<CaptureOptions>,
}

#[derive(Default)]
pub struct CaptureOptions {
    pub out: PathBuf,
    pub size: Option<[f32; 2]>,
    pub view: Option<AxisView>,
    pub ortho: bool,
    /// "shading" (inspector tab), "overlays", "preferences", "welcome", "materials" or "scene":
    /// opens that popover, window or inspector tab.
    pub popover: Option<String>,
    pub pie: bool,
    /// Object index to select.
    pub select: Option<usize>,
    /// Simulated left click at viewport pixel coordinates (exercises GPU picking).
    pub click: Option<[u32; 2]>,
    /// Two measurement points at viewport pixel coordinates (Measure tool).
    pub measure: Option<[[u32; 2]; 2]>,
    /// Section along an axis (0..2) at a fraction of the bounds, optionally flipped.
    pub section: Option<(usize, f32, bool)>,
    /// Model B for an A/B comparison, and whether to use the split layout.
    pub compare: Option<PathBuf>,
    pub compare_split: bool,
    /// Open the UV pane.
    pub uv: bool,
    /// Show this material's texture set in the UV pane.
    pub uv_material: Option<usize>,
    /// Export a turntable (GIF, or MP4 by extension) instead of a screenshot.
    pub turntable: Option<PathBuf>,
    /// Channel to show on the selection (`--select`) only.
    pub channel: Option<TexturePass>,
    pub clip: Option<usize>,
    /// Also runs File > Export Image to this path (with the export preferences).
    pub export: Option<PathBuf>,
    pub frame: Option<i32>,
    /// Move tool: switch to it, rotate the selection (degrees about X, Y, Z), lay it on its
    /// best side, and save the result as STL/3MF.
    pub tool_move: bool,
    pub rotate: Option<[f32; 3]>,
    pub auto_orient: bool,
    pub export_model: Option<PathBuf>,
    /// Lay on face, choosing: the cursor at these viewport pixels.
    pub lay_hover: Option<[u32; 2]>,
    settings: Vec<(String, Option<String>)>,
}

impl CaptureOptions {
    pub fn apply(&self, s: &mut Settings) {
        for (flag, value) in &self.settings {
            let v = value.as_deref().unwrap_or("");
            match flag.as_str() {
                "--shading" => {
                    s.shading = match v {
                        "wireframe" | "wire" => ShadingMode::Wireframe,
                        "rendered" => ShadingMode::Rendered,
                        _ => ShadingMode::Solid,
                    }
                }
                "--lighting" => {
                    s.lighting = match v {
                        "matcap" => Lighting::MatCap,
                        "flat" => Lighting::Flat,
                        _ => Lighting::Studio,
                    }
                }
                "--color" => {
                    s.color = match v {
                        "single" => ColorMode::Single,
                        "random" => ColorMode::Random,
                        "texture" => ColorMode::Texture,
                        "attribute" => ColorMode::Attribute,
                        _ => ColorMode::Material,
                    }
                }
                "--workspace" => {
                    s.auto_workspace = false;
                    s.workspace = if v.starts_with('m') { Workspace::Manufacturing } else { Workspace::Art };
                }
                "--file-materials" => s.file_materials = true,
                "--matcap" => s.matcap = v.parse().unwrap_or(0),
                "--pass" => {
                    s.color = ColorMode::Texture;
                    s.texture_pass = parse_pass(v);
                }
                "--env" => {
                    s.environment = match v {
                        "forest" | "outdoor" => Environment::Preset(Preset::Forest),
                        "studio" => Environment::Preset(Preset::Studio),
                        "sunset" => Environment::Preset(Preset::Sunset),
                        path => Environment::File(path.to_string()),
                    }
                }
                "--env-bg" => s.env_background = true,
                "--export-scale" => s.export_scale = v.parse().unwrap_or(2),
                "--budget" => s.triangle_budget = v.parse().unwrap_or(0),
                "--up" => {
                    s.up_axis = if v.eq_ignore_ascii_case("y") { crate::axes::UpAxis::Y } else { crate::axes::UpAxis::Z }
                }
                "--nav" => {
                    s.navigation = crate::navigation::Navigation::ALL
                        .into_iter()
                        .find(|n| format!("{n:?}").eq_ignore_ascii_case(v))
                        .unwrap_or(s.navigation)
                }
                "--transparent" => s.export_transparent = true,
                "--sidebar" => s.show_sidebar = true,
                "--no-outline" => s.show_outline = false,
                "--normals" => s.show_normals = true,
                "--origins" => s.show_origins = true,
                "--face-orientation" => s.show_face_orientation = true,
                "--mesh-check" => {
                    s.show_non_manifold = true;
                    s.show_open_edges = true;
                    s.show_overlapping = true;
                }
                "--xray" => {
                    s.xray_solid = true;
                    s.xray_wire = true;
                }
                "--no-xray" => {
                    s.xray_solid = false;
                    s.xray_wire = false;
                }
                "--wire-overlay" => s.show_wire_overlay = true,
                "--no-grid" => s.show_grid = false,
                "--fps" => {
                    s.show_fps = true;
                    s.vsync = false;
                }
                _ => {}
            }
        }
    }
}

pub fn parse() -> Result<LaunchOptions, String> {
    let mut args = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned());
    let mut opts = LaunchOptions::default();
    let mut capture = CaptureOptions::default();
    let mut capturing = false;
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--capture" => {
                capture.out = PathBuf::from(value("--capture")?);
                capturing = true;
            }
            "--size" => {
                let v = value("--size")?;
                let (w, h) = v.split_once('x').ok_or("--size expects WxH")?;
                capture.size = Some([
                    w.parse().map_err(|_| "bad --size width")?,
                    h.parse().map_err(|_| "bad --size height")?,
                ]);
            }
            "--view" => {
                capture.view = Some(match value("--view")?.as_str() {
                    "front" => AxisView::Front,
                    "back" => AxisView::Back,
                    "right" => AxisView::Right,
                    "left" => AxisView::Left,
                    "top" => AxisView::Top,
                    "bottom" => AxisView::Bottom,
                    other => return Err(format!("unknown view {other}")),
                })
            }
            "--ortho" => capture.ortho = true,
            "--popover" => capture.popover = Some(value("--popover")?),
            "--pie" => capture.pie = true,
            "--select" => capture.select = value("--select")?.parse().ok(),
            "--clip" => capture.clip = value("--clip")?.parse().ok(),
            "--frame" => capture.frame = value("--frame")?.parse().ok(),
            "--tool" => capture.tool_move = value("--tool")? == "move",
            "--rotate" => {
                let v: Vec<f32> = value("--rotate")?.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                let [x, y, z] = v[..] else { return Err("--rotate expects X,Y,Z degrees".into()) };
                capture.rotate = Some([x, y, z]);
            }
            "--auto-orient" => capture.auto_orient = true,
            "--lay" => {
                let v = value("--lay")?;
                let (x, y) = v.split_once(',').ok_or("--lay expects X,Y")?;
                capture.lay_hover = Some([x.parse().map_err(|_| "bad --lay x")?, y.parse().map_err(|_| "bad --lay y")?]);
            }
            "--export-model" => {
                capture.export_model = Some(PathBuf::from(value("--export-model")?));
                capturing = true;
            }
            "--export" => {
                capture.export = Some(PathBuf::from(value("--export")?));
                capturing = true;
            }
            "--export-scale" | "--nav" | "--budget" | "--up" => {
                let v = value(&arg)?;
                capture.settings.push((arg, Some(v)));
            }
            "--transparent" => capture.settings.push((arg, None)),
            "--channel" => capture.channel = Some(parse_pass(&value("--channel")?)),
            "--compare" => capture.compare = Some(PathBuf::from(value("--compare")?)),
            "--compare-split" => capture.compare_split = true,
            "--uv" => capture.uv = true,
            "--uv-material" => {
                capture.uv = true;
                capture.uv_material = value("--uv-material")?.parse().ok();
            }
            "--turntable" => capture.turntable = Some(PathBuf::from(value("--turntable")?)),
            "--section" => {
                // x|y|z[,position 0..1][,flip]
                let v = value("--section")?;
                let mut parts = v.split(',');
                let axis = match parts.next().unwrap_or("") {
                    "x" | "X" => 0,
                    "y" | "Y" => 1,
                    "z" | "Z" => 2,
                    _ => return Err("--section expects x, y or z".into()),
                };
                let t = parts.next().and_then(|t| t.parse().ok()).unwrap_or(0.5);
                capture.section = Some((axis, t, parts.next() == Some("flip")));
            }
            "--measure" => {
                let v = value("--measure")?;
                let n: Vec<u32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                let [x1, y1, x2, y2] = n[..] else { return Err("--measure expects X1,Y1,X2,Y2".into()) };
                capture.measure = Some([[x1, y1], [x2, y2]]);
            }
            "--click" => {
                let v = value("--click")?;
                let (x, y) = v.split_once(',').ok_or("--click expects X,Y")?;
                capture.click = Some([x.parse().map_err(|_| "bad --click x")?, y.parse().map_err(|_| "bad --click y")?]);
            }
            "--shading" | "--lighting" | "--color" | "--matcap" | "--pass" | "--env" | "--workspace" => {
                let v = value(&arg)?;
                capture.settings.push((arg, Some(v)));
            }
            "--xray" | "--no-xray" | "--wire-overlay" | "--no-grid" | "--fps" | "--env-bg" | "--sidebar"
            | "--file-materials" | "--no-outline" | "--mesh-check" | "--normals" | "--face-orientation" | "--origins" => capture.settings.push((arg, None)),
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            _ => opts.open = Some(PathBuf::from(arg)),
        }
    }
    if capturing {
        opts.capture = Some(capture);
    }
    Ok(opts)
}

fn parse_pass(v: &str) -> TexturePass {
    TexturePass::ALL
        .into_iter()
        .find(|p| p.label().to_ascii_lowercase().replace(' ', "") == v.replace(['-', '_'], ""))
        .unwrap_or(TexturePass::BaseColor)
}
