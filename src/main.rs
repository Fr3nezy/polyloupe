// Hide the console window in release builds on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anim;
mod app;
mod camera;
mod cli;
mod i18n;
mod icon;
mod instance;
mod navigation;
mod qa;
mod loader;
mod render;
mod scene;
mod settings;
mod thumbnail;
mod ui;

use eframe::egui;

fn main() -> eframe::Result {
    env_logger::init();

    // Headless thumbnail mode for the Explorer handler: no window, no saved state.
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|a| a == "--thumbnail-server") {
        if let Err(e) = thumbnail::serve() {
            eprintln!("polyloupe: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
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
    let launch = match cli::parse() {
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
    let size = launch
        .capture
        .as_ref()
        .and_then(|c| c.size)
        .unwrap_or([1280.0, 800.0]);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Poly Loupe")
            .with_decorations(false)
            .with_inner_size(size)
            .with_min_inner_size([640.0, 420.0])
            .with_drag_and_drop(true)
            .with_icon(icon::app_icon()),
        renderer: eframe::Renderer::Wgpu,
        // Capture runs must not move or resize the user's saved window.
        persist_window: launch.capture.is_none(),
        ..Default::default()
    };

    eframe::run_native(
        "Poly Loupe",
        options,
        Box::new(move |cc| Ok(Box::new(app::ViewerApp::new(cc, launch)))),
    )
}

/// Settings saved before the app was renamed live under the old app id: carry them over once.
fn migrate_settings() {
    let (Some(new), Some(old)) = (eframe::storage_dir("Poly Loupe"), eframe::storage_dir("3D Viewer")) else {
        return;
    };
    let (new_file, old_file) = (new.join("app.ron"), old.join("app.ron"));
    if !new_file.exists() && old_file.exists() && std::fs::create_dir_all(&new).is_ok() {
        let _ = std::fs::copy(&old_file, &new_file);
    }
}
