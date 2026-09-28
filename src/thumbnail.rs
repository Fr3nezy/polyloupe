//! Headless thumbnails for the Explorer handler.
//!
//! - `polyloupe --thumbnail IN OUT.png|OUT.rgba [SIZE]`: one thumbnail, then exit (`.rgba` is
//!   raw: width and height as u32 LE, then the pixels).
//! - `polyloupe --thumbnail-server`: keeps the GPU device, pipelines and lighting warm and answers
//!   requests on a loopback port recorded (with a random token) in
//!   `%LOCALAPPDATA%\Poly Loupe\thumbnail-server`. Setting up the GPU is ~90% of a one-shot
//!   thumbnail, so a folder full of models goes ~10x faster. It exits after [`IDLE_EXIT`].
//!
//! Protocol, one request per connection: `TOKEN\nSIZE\nPATH\n`, answered with
//! `ok W H\n` + W*H*4 straight-alpha RGBA bytes, or `err MESSAGE\n`.
//!
//! Thumbnails render on a transparent background, from a Blender-like 3/4 view fitted tightly to
//! the model.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui_wgpu::wgpu;
use glam::{Vec3, Vec4Swizzles};

use crate::camera::Camera;
use crate::loader;
use crate::render::{FrameInput, Renderer, environment};
use crate::scene::{Aabb, Scene};
use crate::settings::{ColorMode, Settings, ShadingMode};

const IDLE_EXIT: Duration = Duration::from_secs(60);

pub fn run(input: &Path, output: &Path, size: u32) -> Result<(), String> {
    let size = size.clamp(16, 1024);
    let scene = loader::load(input)?;
    let (w, h, rgba) = Thumbnailer::new()?.render(&scene, size)?;
    if output.extension().is_some_and(|e| e == "rgba") {
        // Raw for the Explorer handler: width and height (u32 LE), then RGBA bytes.
        let mut raw = Vec::with_capacity(8 + rgba.len());
        raw.extend_from_slice(&w.to_le_bytes());
        raw.extend_from_slice(&h.to_le_bytes());
        raw.extend_from_slice(&rgba);
        return std::fs::write(output, raw).map_err(|e| e.to_string());
    }
    image::save_buffer(output, &rgba, w, h, image::ColorType::Rgba8).map_err(|e| e.to_string())
}

/// GPU state reused across thumbnails.
struct Thumbnailer {
    renderer: Renderer,
    lit: bool,
}

impl Thumbnailer {
    fn new() -> Result<Self, String> {
        let (device, queue) = headless_device()?;
        Ok(Self { renderer: Renderer::new(&device, &queue), lit: false })
    }

    fn render(&mut self, scene: &Scene, size: u32) -> Result<(u32, u32, Vec<u8>), String> {
        let size = size.clamp(16, 1024);
        self.renderer.upload_scene(scene);
        let settings = thumbnail_settings(scene);
        if settings.shading == ShadingMode::Rendered && !self.lit {
            self.renderer.set_environment(&environment::load(environment::Preset::Forest));
            self.lit = true;
        }
        // Supersample: render at 2x, then box-filter down for clean edges.
        let render_size = (size * 2).min(2048);
        let camera = frame_camera(&scene.bounds, &sample_points(scene));
        let input = FrameInput {
            view: camera.view_matrix(),
            proj: camera.projection(1.0),
            eye: camera.eye(),
            cam_back: camera.back(),
            ortho: false,
            grid_cell: 1.0,
            grid_fade: 0.0,
            grid_axis: 2,
            settings: &settings,
            pick: None,
            transparent: true,
            section: None,
            normal_length: 0.0,
        };
        self.renderer.render(None, [render_size, render_size], &input);
        let ([w, h], pixels) = self.renderer.read_pixels().ok_or("Couldn't read the rendered image")?;
        Ok((size, size, downsample(&pixels, w, h, size)))
    }
}

fn server_record() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Poly Loupe").join("thumbnail-server"))
}

/// Runs the thumbnail server until it has been idle for [`IDLE_EXIT`].
pub fn serve() -> Result<(), String> {
    let record = server_record().ok_or("LOCALAPPDATA isn't set")?;
    // Several Explorer threads may start a server at once: the first one to answer wins.
    if let Ok(text) = std::fs::read_to_string(&record) {
        if let Some(port) = text.split_whitespace().next().and_then(|p| p.parse::<u16>().ok()) {
            let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
                return Ok(());
            }
        }
    }
    // Publish the port before the (slow) GPU setup: clients connect right away and wait in the
    // listen backlog, so parallel requests don't each start their own server.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = token();
    if let Some(dir) = record.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&record, format!("{port} {token}")).map_err(|e| e.to_string())?;
    let thumbnailer = match Thumbnailer::new() {
        Ok(t) => Arc::new(Mutex::new(t)),
        Err(e) => {
            let _ = std::fs::remove_file(&record);
            return Err(e);
        }
    };

    let start = Instant::now();
    let last_request = Arc::new(AtomicU64::new(0));
    {
        let (last, record, token) = (last_request.clone(), record.clone(), token.clone());
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let idle = start.elapsed().saturating_sub(Duration::from_millis(last.load(Ordering::Relaxed)));
                if idle > IDLE_EXIT {
                    if std::fs::read_to_string(&record).is_ok_and(|t| t.ends_with(&token)) {
                        let _ = std::fs::remove_file(&record);
                    }
                    std::process::exit(0);
                }
            }
        });
    }
    for stream in listener.incoming().flatten() {
        last_request.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
        let (thumbnailer, token, last) = (thumbnailer.clone(), token.clone(), last_request.clone());
        std::thread::spawn(move || {
            handle(stream, &token, &thumbnailer);
            last.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
        });
    }
    Ok(())
}

fn handle(stream: TcpStream, token: &str, thumbnailer: &Mutex<Thumbnailer>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut lines = BufReader::new(&stream).lines();
    let (Some(Ok(t)), Some(Ok(size)), Some(Ok(path))) = (lines.next(), lines.next(), lines.next()) else { return };
    if t != token {
        return;
    }
    let size = size.parse().unwrap_or(256);
    // Parsing runs in parallel on each connection's thread; only the GPU part is serialized.
    let result = loader::load(Path::new(&path)).and_then(|scene| match thumbnailer.lock() {
        Ok(mut t) => t.render(&scene, size),
        Err(_) => Err("thumbnailer poisoned".into()),
    });
    let mut out = &stream;
    let _ = match result {
        Ok((w, h, rgba)) => out.write_all(format!("ok {w} {h}\n").as_bytes()).and_then(|_| out.write_all(&rgba)),
        Err(e) => out.write_all(format!("err {}\n", e.replace('\n', " ")).as_bytes()),
    };
}

fn token() -> String {
    use std::hash::BuildHasher;
    let seed = (std::process::id(), std::time::SystemTime::now());
    format!("{:016x}", std::collections::hash_map::RandomState::new().hash_one(seed))
}

fn headless_device() -> Result<(wgpu::Device, wgpu::Queue), String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    // The integrated GPU wakes up faster and is plenty for small frames.
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::LowPower),
        ..Default::default()
    }))
    .map_err(|e| format!("No GPU adapter: {e}"))?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        .map_err(|e| format!("Couldn't create a GPU device: {e}"))
}

/// Textured models look best lit like Material Preview; plain ones like Blender's Solid mode.
fn thumbnail_settings(scene: &Scene) -> Settings {
    let textured = scene.materials.iter().any(|m| m.base_color_tex.is_some());
    Settings {
        shading: if textured { ShadingMode::Rendered } else { ShadingMode::Solid },
        color: ColorMode::Material,
        show_grid: false,
        show_axes: false,
        show_outline: false,
        show_wire_overlay: false,
        xray_solid: false,
        env_background: false,
        ..Settings::default()
    }
}

/// 3/4 view like Blender's default, adjusted for flat or tall models, then fitted so the
/// bounding box fills the frame with a small margin.
fn frame_camera(bounds: &Aabb, points: &[Vec3]) -> Camera {
    let mut cam = Camera::default();
    cam.fov_y = 30f32.to_radians();
    if !bounds.is_valid() {
        return cam;
    }
    let size = bounds.size();
    let footprint = size.x.max(size.y).max(1e-6);
    cam.view.yaw = (-60f32).to_radians();
    cam.view.pitch = if size.z < footprint * 0.15 {
        // Flat things (tiles, cards, terrain) read better from above.
        55f32.to_radians()
    } else if size.z > footprint * 3.0 {
        // Tall things (characters, towers) from closer to eye level.
        12f32.to_radians()
    } else {
        25f32.to_radians()
    };
    cam.frame(bounds, false);
    cam.scene_radius = bounds.radius().max(1e-4);

    // Fit the actual geometry (sampled), not the bounding box corners, so the model fills the
    // frame instead of the empty parts of its box.
    let corners: Vec<Vec3> = if points.is_empty() {
        (0..8)
            .map(|i| {
                Vec3::new(
                    if i & 1 == 0 { bounds.min.x } else { bounds.max.x },
                    if i & 2 == 0 { bounds.min.y } else { bounds.max.y },
                    if i & 4 == 0 { bounds.min.z } else { bounds.max.z },
                )
            })
            .collect()
    } else {
        points.to_vec()
    };
    const FILL: f32 = 0.86;
    for _ in 0..6 {
        let view_proj = cam.projection(1.0) * cam.view_matrix();
        let (mut lo, mut hi) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
        for c in &corners {
            let clip = view_proj * c.extend(1.0);
            let ndc = clip.xy() / clip.w.max(1e-6);
            lo = lo.min(ndc);
            hi = hi.max(ndc);
        }
        // Center the projected box, then scale the distance so its larger side fills FILL.
        let center = (lo + hi) * 0.5;
        let half_h = cam.view.distance * (cam.fov_y * 0.5).tan();
        cam.view.target += (cam.right() * center.x + cam.up() * center.y) * half_h;
        let extent = ((hi - lo) * 0.5).max_element();
        cam.view.distance *= (extent / FILL).clamp(0.2, 5.0);
    }
    cam
}

/// Box filter from `w`x`h` to `size`x`size` (premultiplied so edges don't darken).
fn downsample(src: &[u8], w: u32, h: u32, size: u32) -> Vec<u8> {
    let (w, h, size) = (w as usize, h as usize, size as usize);
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        let (y0, y1) = (y * h / size, ((y + 1) * h / size).max(y * h / size + 1));
        for x in 0..size {
            let (x0, x1) = (x * w / size, ((x + 1) * w / size).max(x * w / size + 1));
            let mut acc = [0f32; 4];
            let mut n = 0f32;
            for sy in y0..y1.min(h) {
                for sx in x0..x1.min(w) {
                    let p = &src[(sy * w + sx) * 4..(sy * w + sx) * 4 + 4];
                    let a = p[3] as f32 / 255.0;
                    acc[0] += p[0] as f32 * a;
                    acc[1] += p[1] as f32 * a;
                    acc[2] += p[2] as f32 * a;
                    acc[3] += a;
                    n += 1.0;
                }
            }
            let o = (y * size + x) * 4;
            if acc[3] > 0.0 {
                for c in 0..3 {
                    out[o + c] = (acc[c] / acc[3]).round().clamp(0.0, 255.0) as u8;
                }
            }
            out[o + 3] = (acc[3] / n.max(1.0) * 255.0).round() as u8;
        }
    }
    out
}

/// Up to ~60k world-space vertex positions spread over every mesh.
fn sample_points(scene: &Scene) -> Vec<Vec3> {
    let total: usize = scene.meshes.iter().map(|m| m.positions.len()).sum();
    let stride = (total / 60_000).max(1);
    scene
        .meshes
        .iter()
        .flat_map(|m| m.positions.iter().step_by(stride).map(move |p| m.transform.transform_point3(Vec3::from(*p))))
        .collect()
}
