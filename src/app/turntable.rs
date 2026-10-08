//! Turntable export (File > Export Turntable): one full turn around the model from the
//! current view, as a looping GIF (encoded here) or an MP4 (through ffmpeg, when installed).
//! An animated model can play its current clip once over the turn.

use std::io::Write as _;
use std::process::{Child, Command, Stdio};

use super::*;

impl ViewerApp {
    pub(super) fn turntable_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_turntable;
        let mut export = false;
        egui::Window::new(tr("Export Turntable"))
            .id(egui::Id::new("turntable"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(340.0)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let s = &mut self.settings;
                widgets::section(ui, "Format");
                ui.horizontal(|ui| widgets::segmented(ui, &mut s.turntable_mp4, &[(false, "GIF"), (true, "MP4")]));
                if s.turntable_mp4 && !ffmpeg_available() {
                    ui.label(
                        RichText::new(tr("MP4 needs ffmpeg on the PATH (winget install ffmpeg). GIF works without it."))
                            .size(11.0)
                            .color(theme::ERROR),
                    );
                }
                widgets::section(ui, "Size");
                ui.horizontal(|ui| widgets::segmented(ui, &mut s.turntable_size, &[(480, "480"), (720, "720"), (1080, "1080")]))
                    .response
                    .on_hover_text(tr("Long side in pixels; the shape follows the view"));
                ui.add(egui::Slider::new(&mut s.turntable_seconds, 2.0..=12.0).step_by(0.5).suffix(" s").text(tr("Length")));
                ui.add_enabled_ui(self.anim.as_ref().is_some_and(|a| a.has_clips()), |ui| {
                    ui.checkbox(&mut s.turntable_animate, tr("Play the animation during the turn"));
                });
                widgets::section(ui, "Background");
                ui.checkbox(&mut s.export_transparent, tr("Transparent background"))
                    .on_hover_text(tr("Export with alpha transparency (best with GIF)"));
                if s.turntable_mp4 && s.export_transparent {
                    ui.label(
                        RichText::new(tr("MP4 does not support transparency; export as GIF for transparent alpha."))
                            .size(11.0)
                            .color(theme::TEXT_DIM),
                    );
                }
                ui.add_space(6.0);
                ui.label(RichText::new(tr("Turns once around the model from the current view, with the current shading.")).size(11.0).color(theme::TEXT_DIM));
                ui.add_space(6.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let busy = self.turntable_job.is_some();
                    if ui.add_enabled(!busy, egui::Button::new(tr("Export…"))).clicked() {
                        export = true;
                    }
                    if busy {
                        ui.add(egui::Spinner::new().size(14.0));
                    }
                });
            });
        self.show_turntable = open;
        if export {
            self.export_turntable(ctx);
        }
    }

    pub(super) fn poll_turntable(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.turntable_job else { return };
        let Ok(result) = rx.try_recv() else { return };
        self.turntable_job = None;
        match result {
            Ok(path) => self.show_toast(ctx, trf("Saved {name}", &[("name", &file_name(&path))]), false),
            Err(e) => self.show_toast(ctx, e, true),
        }
    }

    fn export_turntable(&mut self, ctx: &egui::Context) {
        let mp4 = self.settings.turntable_mp4;
        if mp4 && !ffmpeg_available() {
            self.show_toast(ctx, tr("MP4 needs ffmpeg on the PATH (winget install ffmpeg). GIF works without it.").to_string(), true);
            return;
        }
        let (ext, filter) = if mp4 { ("mp4", "MP4 video") } else { ("gif", "GIF animation") };
        let stem = self.info.as_ref().map_or("turntable".to_string(), |i| {
            Path::new(&i.file_name).file_stem().map_or(i.file_name.clone(), |s| s.to_string_lossy().into_owned())
        });
        let Some(path) = rfd::FileDialog::new()
            .set_title(tr("Export Turntable"))
            .add_filter(tr(filter), &[ext])
            .set_file_name(format!("{stem}_turntable.{ext}"))
            .save_file()
        else {
            return;
        };
        self.export_turntable_to(path.with_extension(ext), ctx);
    }

    /// Renders and encodes to `path` (format from the settings); encoding runs on a thread.
    pub(super) fn export_turntable_to(&mut self, path: PathBuf, ctx: &egui::Context) {
        let mp4 = self.settings.turntable_mp4;

        // Frame size: the view's shape, long side as chosen, even (H.264 needs it).
        let [vw, vh] = self.viewport_px;
        let long = self.settings.turntable_size as f32;
        let k = long / vw.max(vh).max(1) as f32;
        let even = |v: f32| ((v / 2.0).round() as u32 * 2).max(2);
        let size = [even(vw as f32 * k), even(vh as f32 * k)];
        let fps: u32 = if mp4 { 30 } else { 25 };
        let frames = (self.settings.turntable_seconds * fps as f32).round().max(2.0) as u32;

        let mut encoder = if mp4 {
            match spawn_ffmpeg(&path, size, fps) {
                Ok(child) => Some(child),
                Err(e) => {
                    self.show_toast(ctx, e, true);
                    return;
                }
            }
        } else {
            None
        };

        // Render every frame now (blocking briefly); GIF frames are kept for the encoder
        // thread, MP4 frames stream straight into ffmpeg.
        let start = self.camera.view;
        // Turn around the model's center (the view may be aimed elsewhere), same distance.
        let bounds = self.visible_bounds(false);
        if bounds.is_valid() {
            self.camera.view.target = bounds.center();
        }
        let animate = self.settings.turntable_animate;
        let mut gif_frames = Vec::new();
        let mut error = None;
        for i in 0..frames {
            let t = i as f32 / frames as f32;
            self.camera.view.yaw = start.yaw + std::f32::consts::TAU * t;
            if animate {
                if let (Some(p), Some(r)) = (&mut self.anim, &mut self.renderer) {
                    p.time = p.duration() * t;
                    let pose = p.pose();
                    r.set_object_transforms(&pose.object_transforms);
                    r.set_joints(&pose.joints);
                    for (m, positions, normals) in &pose.morphed {
                        r.update_mesh_geometry(*m, positions, normals);
                    }
                }
            }
            let transparent = self.settings.export_transparent || self.settings.transparent_background;
            let Some((_, pixels)) = self.render_offscreen(size, transparent) else {
                error = Some(tr("Couldn't render the image").to_string());
                break;
            };
            match &mut encoder {
                Some(child) => {
                    let ok = child.stdin.as_mut().is_some_and(|stdin| stdin.write_all(&pixels).is_ok());
                    if !ok {
                        error = Some(tr("ffmpeg stopped while encoding").to_string());
                        break;
                    }
                }
                None => gif_frames.push(pixels),
            }
        }
        self.camera.view = start;
        self.anim_dirty = true;
        if let Some(e) = error {
            if let Some(mut child) = encoder {
                let _ = child.kill();
            }
            self.show_toast(ctx, e, true);
            return;
        }

        let (tx, rx) = channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = match encoder {
                Some(child) => finish_ffmpeg(child).map(|()| path),
                None => write_gif(&path, size, fps, gif_frames).map(|()| path),
            };
            let _ = tx.send(result);
            repaint.request_repaint();
        });
        self.turntable_job = Some(rx);
        self.show_toast(ctx, tr("Encoding the turntable…").to_string(), false);
    }
}

fn ffmpeg_command() -> Command {
    let mut cmd = Command::new("ffmpeg");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing up.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn ffmpeg_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        ffmpeg_command().arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    })
}

fn spawn_ffmpeg(path: &Path, [w, h]: [u32; 2], fps: u32) -> Result<Child, String> {
    ffmpeg_command()
        .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgba", "-s"])
        .arg(format!("{w}x{h}"))
        .args(["-r", &fps.to_string(), "-i", "-", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "18", "-movflags", "+faststart"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| trf("Couldn't start ffmpeg: {error}", &[("error", &e)]))
}

fn finish_ffmpeg(mut child: Child) -> Result<(), String> {
    drop(child.stdin.take());
    match child.wait() {
        Ok(s) if s.success() => Ok(()),
        _ => Err(tr("ffmpeg couldn't encode the video").to_string()),
    }
}

fn write_gif(path: &Path, [w, h]: [u32; 2], fps: u32, frames: Vec<Vec<u8>>) -> Result<(), String> {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame, RgbaImage};
    let fail = |e: &dyn std::fmt::Display| trf("Couldn't save the image: {error}", &[("error", &e)]);
    let file = std::fs::File::create(path).map_err(|e| fail(&e))?;
    let mut encoder = GifEncoder::new_with_speed(std::io::BufWriter::new(file), 10);
    encoder.set_repeat(Repeat::Infinite).map_err(|e| fail(&e))?;
    let delay = Delay::from_numer_denom_ms(1000, fps);
    for pixels in frames {
        let image = RgbaImage::from_raw(w, h, pixels).ok_or_else(|| fail(&"frame size"))?;
        encoder.encode_frame(Frame::from_parts(image, 0, 0, delay)).map_err(|e| fail(&e))?;
    }
    Ok(())
}
