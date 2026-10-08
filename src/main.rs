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
mod update;
mod uv;

// Scene model and loaders live in polyloupe-core, shared with the thumbnail handler.
use polyloupe_core::{loader, scene};

use eframe::{egui, egui_wgpu};

/// When the process started, for the startup timings in the log (`RUST_LOG=polyloupe=info`).
pub static LAUNCHED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Logs how long after launch a startup step finished.
pub fn startup_mark(step: &str) {
    if let Some(t) = LAUNCHED.get() {
        log::info!("startup: {step} at {} ms", t.elapsed().as_millis());
    }
}

fn main() -> eframe::Result {
    // The "opened in" time of a file opened at launch counts from here.
    let launched = std::time::Instant::now();
    let _ = LAUNCHED.set(launched);
    env_logger::Builder::from_default_env().format_timestamp_millis().init();
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
    // A model opened at launch loads while the window starts, before the settings are read:
    // pick the language now so its warnings come out translated.
    i18n::set(saved_language());

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

/// GPU setup. Vulkan first, on its own: creating the instance and device ourselves keeps wgpu
/// from also enumerating DX12 adapters (it creates a D3D12 device per GPU, ~200 ms at every
/// launch on a hybrid laptop), and Vulkan builds our pipelines faster than DX12's shader
/// compiler. Machines without a usable Vulkan driver get DX12 (whose WARP software renderer also
/// covers GPUs without drivers). `WGPU_BACKEND` still picks the backend by hand.
fn wgpu_setup() -> egui_wgpu::WgpuSetup {
    use egui_wgpu::wgpu;
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    // wgpu's default 256 MB buffer cap rejects big scans and CAD exports (a 5.6M triangle STL
    // needs ~270 MB per attribute buffer): allow whatever the GPU supports.
    let default_descriptor = setup.device_descriptor.clone();
    setup.device_descriptor = std::sync::Arc::new(move |adapter| {
        let mut desc = default_descriptor(adapter);
        desc.required_limits.max_buffer_size = adapter.limits().max_buffer_size;
        desc
    });
    setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters, surface| {
        adapters
            .iter()
            .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
            .max_by_key(|a| adapter_rank(a))
            .cloned()
            .ok_or_else(|| "no GPU adapter can draw to this window".to_string())
    }));
    if wgpu::Backends::from_env().is_some() {
        return egui_wgpu::WgpuSetup::CreateNew(setup);
    }

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags: setup.instance_descriptor.flags,
        backend_options: setup.instance_descriptor.backend_options.clone(),
        memory_budget_thresholds: setup.instance_descriptor.memory_budget_thresholds,
        display: None,
    });
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    if let Some(adapter) = adapters.iter().max_by_key(|a| adapter_rank(a)).cloned() {
        match pollster::block_on(adapter.request_device(&(setup.device_descriptor)(&adapter))) {
            Ok((device, queue)) => {
                return egui_wgpu::WgpuSetup::Existing(egui_wgpu::WgpuSetupExisting { instance, adapter, device, queue });
            }
            Err(e) => log::warn!("Vulkan device failed ({e}), trying DX12"),
        }
    }
    setup.instance_descriptor.backends = wgpu::Backends::DX12;
    egui_wgpu::WgpuSetup::CreateNew(setup)
}

/// Discrete GPU first (integrated with `WGPU_POWER_PREF=low`), then Vulkan over DX12.
fn adapter_rank(a: &egui_wgpu::wgpu::Adapter) -> (u8, u8) {
    use egui_wgpu::wgpu::{Backend, DeviceType, PowerPreference};
    let low_power = PowerPreference::from_env() == Some(PowerPreference::LowPower);
    let info = a.get_info();
    let device = match info.device_type {
        DeviceType::DiscreteGpu => if low_power { 2 } else { 3 },
        DeviceType::IntegratedGpu => if low_power { 3 } else { 2 },
        DeviceType::VirtualGpu => 1,
        _ => 0,
    };
    let backend = match info.backend {
        Backend::Vulkan => 2,
        Backend::Dx12 => 1,
        _ => 0,
    };
    (device, backend)
}

/// Settings saved before the app was renamed live under an old app id ("Poly Loupe", before that
/// "3D Viewer"): carry the newest over once.
/// The interface language saved in the settings, read straight from eframe's `app.ron` (the
/// settings are nested in it as an escaped RON string, so a plain search does). The app sets the
/// language again from its settings once the window exists.
fn saved_language() -> i18n::Language {
    let saved = eframe::storage_dir("PolyLoupe").and_then(|d| std::fs::read_to_string(d.join("app.ron")).ok());
    let value = saved.as_deref().and_then(|s| {
        let rest = s[s.find("language:")? + "language:".len()..].trim_start();
        Some(rest[..rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len())].to_string())
    });
    match value.as_deref() {
        Some("English") => i18n::Language::English,
        Some("Italian") => i18n::Language::Italian,
        _ => i18n::Language::System,
    }
}

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
