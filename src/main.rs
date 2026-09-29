// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anim;
mod axes;
mod app;
mod camera;
mod cli;
mod cloak;
mod i18n;
mod icon;
mod instance;
mod navigation;
mod qa;
mod render;
mod settings;
mod snap;
mod thumbnail;
mod ui;
mod uv;

// Scene model and loaders live in polyloupe-core, shared with the thumbnail handler.
use polyloupe_core::{loader, scene};

use eframe::{egui, egui_wgpu};

fn main() -> eframe::Result {
    // The "opened in" time of a file opened at launch counts from here.
    let launched = std::time::Instant::now();
    env_logger::init();
    polyloupe_core::i18n::set_translator(|en| i18n::tr(en).to_string());

    // The Explorer thumbnail, rendered headless (no window, no saved state), to check what
    // Explorer will show.
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--thumbnail") {
        let (Some(input), Some(output)) = (args.get(2), args.get(3)) else {
            eprintln!("usage: polyloupe --thumbnail INPUT OUTPUT.png [SIZE]");
            std::process::exit(2);
        };
        let size = args.get(4).and_then(|s| s.to_str()?.parse().ok()).unwrap_or(256);
        if let Err(e) = thumbnail::run(input.as_ref(), output.as_ref(), size) {
            eprintln!("polyloupe: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }

    // `polyloupe model.glb` is also what Windows runs when the app is the file's default handler.
    let mut launch = match cli::parse() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("polyloupe: {e}");
            std::process::exit(2);
        }
    };
    migrate_settings();

    // With "Open files in the same window" on, the running window takes the file instead.
    if launch.capture.is_none() && launch.open.as_deref().is_some_and(instance::forward) {
        return Ok(());
    }
    // Read the model while the window and the GPU get ready.
    let preload = launch
        .open
        .take_if(|p| loader::is_supported(p))
        .map(|p| app::Preload::start(p, launched));
    let size = launch
        .capture
        .as_ref()
        .and_then(|c| c.size)
        .unwrap_or([1280.0, 800.0]);

    let capture = launch.capture.is_some();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PolyLoupe")
            .with_decorations(false)
            .with_inner_size(size)
            .with_min_inner_size([640.0, 420.0])
            .with_drag_and_drop(true)
            .with_icon(icon::app_icon()),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: egui_wgpu::WgpuConfiguration { wgpu_setup: wgpu_setup(), ..Default::default() },
        // Capture runs must not move or resize the user's saved window.
        persist_window: !capture,
        // eframe keeps the window hidden until its first frame is painted, but Windows shows a
        // window the moment it is maximized: restore it un-maximized and maximize it on that
        // first frame (app::RESTORE_MAXIMIZED), so it never appears empty. Captures ignore the
        // saved window and use their own size.
        window_builder: Some(Box::new(move |builder| {
            if capture {
                builder.with_maximized(false).with_inner_size(size)
            } else if builder.maximized == Some(true) {
                app::RESTORE_MAXIMIZED.store(true, std::sync::atomic::Ordering::Relaxed);
                builder.with_maximized(false)
            } else {
                builder
            }
        })),
        ..Default::default()
    };

    eframe::run_native(
        "PolyLoupe",
        options,
        Box::new(move |cc| Ok(Box::new(app::ViewerApp::new(cc, launch, preload)))),
    )
}

/// Vulkan and DX12 only (GL costs startup time, and on Windows DX12 already falls back to its WARP
/// software renderer on GPUs without drivers), and the adapter
/// picked from the list egui-wgpu already made: its default path lets wgpu enumerate every GPU a
/// second time, which on a hybrid laptop wakes both GPUs again (~0.25 s each round).
fn wgpu_setup() -> egui_wgpu::WgpuSetup {
    use egui_wgpu::wgpu::{self, Backend, DeviceType, PowerPreference};
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    if wgpu::Backends::from_env().is_none() {
        setup.instance_descriptor.backends = wgpu::Backends::PRIMARY;
    }
    setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters, surface| {
        let low_power = PowerPreference::from_env() == Some(PowerPreference::LowPower);
        let rank = |a: &wgpu::Adapter| {
            let info = a.get_info();
            let device = match info.device_type {
                DeviceType::DiscreteGpu => if low_power { 2 } else { 3 },
                DeviceType::IntegratedGpu => if low_power { 3 } else { 2 },
                DeviceType::VirtualGpu => 1,
                _ => 0,
            };
            // Vulkan builds our pipelines much faster than DX12's shader compiler.
            let backend = match info.backend {
                Backend::Vulkan => 2,
                Backend::Dx12 => 1,
                _ => 0,
            };
            (device, backend)
        };
        adapters
            .iter()
            .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
            .max_by_key(|a| rank(a))
            .cloned()
            .ok_or_else(|| "no GPU adapter can draw to this window".to_string())
    }));
    egui_wgpu::WgpuSetup::CreateNew(setup)
}

/// Settings saved before the app was renamed live under an old app id ("Poly Loupe", before that
/// "3D Viewer"): carry the newest over once.
fn migrate_settings() {
    let Some(new) = eframe::storage_dir("PolyLoupe") else { return };
    let new_file = new.join("app.ron");
    if new_file.exists() {
        return;
    }
    let old_file = ["Poly Loupe", "3D Viewer"]
        .into_iter()
        .filter_map(|id| eframe::storage_dir(id).map(|d| d.join("app.ron")))
        .find(|f| f.exists());
    if let Some(old_file) = old_file {
        if std::fs::create_dir_all(&new).is_ok() {
            let _ = std::fs::copy(&old_file, &new_file);
        }
    }
}
