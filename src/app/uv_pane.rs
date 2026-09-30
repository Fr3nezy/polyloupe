//! UV layout pane (U): docked on the right of the 3D view, so it never covers the model.
//! Shows the UV edges of the selected objects (or all visible ones), or of every object using
//! one material (its texture set), over the 0..1 square and, optionally, the base color texture; mirrored faces are filled red. Wheel zooms at the
//! cursor, any drag pans, double-click fits. The layout is rasterized on a worker thread for
//! the visible window only (see `uv::rasterize`).

use std::sync::Arc;

use eframe::egui::{CursorIcon, Id, TextFormat, Vec2, text::LayoutJob};

use super::*;
use crate::i18n::thousands;
use crate::ui::widgets::text_button;
use crate::uv::{self, UvData, Window};

/// Part of the viewport width the pane starts with.
const DEFAULT_WIDTH: f32 = 0.42;
const HEADER: f32 = 40.0;

type RasterKey = (Vec<usize>, [u32; 4], [usize; 2]);

pub(super) struct UvView {
    pub(super) open: bool,
    center: [f32; 2],
    zoom: f32,
    show_texture: bool,
    /// Texture set shown instead of the selection: every object using this material. Picking one
    /// selects those objects; changing the selection afterwards goes back to following it.
    material: Option<usize>,
    material_selection: Vec<bool>,
    /// Pane width as a fraction of the viewport.
    width: f32,
    raster: Option<(RasterKey, Window, TextureHandle)>,
    job: Option<(RasterKey, Window, Receiver<Vec<u8>>, [usize; 2])>,
    textures: HashMap<usize, TextureHandle>,
}

impl Default for UvView {
    fn default() -> Self {
        Self {
            open: false,
            center: [0.5, 0.5],
            zoom: 1.0,
            show_texture: true,
            material: None,
            material_selection: Vec::new(),
            width: DEFAULT_WIDTH,
            raster: None,
            job: None,
            textures: HashMap::new(),
        }
    }
}

impl UvView {
    /// New model: drop cached images, keep the pane's layout.
    pub(super) fn reset(&mut self) {
        self.raster = None;
        self.job = None;
        self.textures.clear();
        self.material = None;
        self.center = [0.5, 0.5];
        self.zoom = 1.0;
    }
}

impl ViewerApp {
    /// Splits the viewport into the 3D view and the UV pane when it's open.
    pub(super) fn uv_split(&self, rect: Rect) -> (Rect, Option<Rect>) {
        if !self.uv_view.open || self.info.is_none() {
            return (rect, None);
        }
        let x = (rect.right() - rect.width() * self.uv_view.width).round();
        (Rect::from_min_max(rect.min, pos2(x - 1.0, rect.max.y)), Some(Rect::from_min_max(pos2(x, rect.min.y), rect.max)))
    }

    pub(super) fn uv_pane(&mut self, ui: &mut Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let painter = ui.painter().with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(0x1a, 0x1a, 0x1a));
        painter.vline(rect.left() - 0.5, rect.y_range(), Stroke::new(2.0, theme::BG_APP));

        // Resize by dragging the pane's left edge.
        let edge = Rect::from_center_size(pos2(rect.left(), rect.center().y), vec2(8.0, rect.height()));
        let r = ui.interact(edge, Id::new("uv_edge"), Sense::drag());
        if r.hovered() || r.dragged() {
            ctx.set_cursor_icon(CursorIcon::ResizeHorizontal);
        }
        if let (true, Some(p)) = (r.dragged(), r.interact_pointer_pos()) {
            let full = self.viewport_full_rect;
            self.uv_view.width = ((full.right() - p.x) / full.width()).clamp(0.2, 0.75);
        }

        let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER));
        let canvas = Rect::from_min_max(pos2(rect.left(), header.bottom()), rect.max);
        let Some(data) = self.uv.clone() else {
            self.uv_header(ui, header, None, &[]);
            painter.text(canvas.center(), Align2::CENTER_CENTER, tr("Analyzing…"), FontId::proportional(13.0), theme::TEXT_DIM);
            return;
        };
        if self.uv_view.material.is_some() && self.uv_view.material_selection != self.selection.selected {
            self.uv_view.material = None;
        }
        let selected: Vec<usize> = (0..self.visible.len()).filter(|&i| self.selection.selected[i]).collect();
        let pool = match (self.uv_view.material, &self.info) {
            (Some(m), Some(info)) => (0..info.objects.len()).filter(|&i| info.objects[i].material == m).collect(),
            _ if selected.is_empty() => (0..self.visible.len()).filter(|&i| self.visible[i]).collect(),
            _ => selected,
        };
        let targets: Vec<usize> = pool.into_iter().filter(|&i| data.meshes.get(i).is_some_and(|m| m.is_some())).collect();
        self.uv_header(ui, header, Some(&data), &targets);

        // View transform: UV (v down) to screen.
        let v = &mut self.uv_view;
        let scale = v.zoom * canvas.width().min(canvas.height()) * 0.9;
        let r = ui.interact(canvas, Id::new("uv_canvas"), Sense::click_and_drag());
        if r.dragged() {
            let d = r.drag_delta();
            v.center[0] -= d.x / scale;
            v.center[1] -= d.y / scale;
        }
        if r.double_clicked() {
            v.center = [0.5, 0.5];
            v.zoom = 1.0;
        }
        if r.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                if let Some(p) = r.hover_pos() {
                    // Zoom about the cursor: the UV point under it stays put.
                    let under = [v.center[0] + (p.x - canvas.center().x) / scale, v.center[1] + (p.y - canvas.center().y) / scale];
                    let k = (scroll / 200.0).exp();
                    v.zoom = (v.zoom * k).clamp(0.05, 200.0);
                    let s2 = v.zoom * canvas.width().min(canvas.height()) * 0.9;
                    v.center = [under[0] - (p.x - canvas.center().x) / s2, under[1] - (p.y - canvas.center().y) / s2];
                }
            }
        }
        let scale = v.zoom * canvas.width().min(canvas.height()) * 0.9;
        let to_screen = |u: f32, w: f32| pos2(canvas.center().x + (u - v.center[0]) * scale, canvas.center().y + (w - v.center[1]) * scale);
        let window = Window {
            min: [v.center[0] - canvas.width() * 0.5 / scale, v.center[1] - canvas.height() * 0.5 / scale],
            max: [v.center[0] + canvas.width() * 0.5 / scale, v.center[1] + canvas.height() * 0.5 / scale],
        };
        let painter = ui.painter().with_clip_rect(canvas);

        // 0..1 square: texture or a flat tile, then the grid.
        let square = Rect::from_two_pos(to_screen(0.0, 0.0), to_screen(1.0, 1.0));
        let material = match v.material {
            Some(m) => Some(m),
            None => self.selection.active.or(targets.first().copied()).and_then(|i| data.meshes[i].as_ref()).map(|m| m.material),
        };
        let texture = material.filter(|_| v.show_texture).and_then(|m| {
            let (size, pixels) = data.textures.get(m)?.as_ref()?;
            Some(
                v.textures
                    .entry(m)
                    .or_insert_with(|| {
                        ctx.load_texture(format!("uv_tex_{m}"), egui::ColorImage::from_rgba_unmultiplied(*size, pixels), TextureOptions::LINEAR)
                    })
                    .id(),
            )
        });
        match texture {
            Some(t) => {
                painter.image(t, square, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::from_gray(150));
            }
            None => {
                painter.rect_filled(square, 0.0, Color32::from_rgb(0x24, 0x24, 0x24));
            }
        }
        for k in 1..8 {
            let t = k as f32 / 8.0;
            let stroke = Stroke::new(1.0, Color32::from_white_alpha(if k == 4 { 26 } else { 12 }));
            painter.line_segment([to_screen(t, 0.0), to_screen(t, 1.0)], stroke);
            painter.line_segment([to_screen(0.0, t), to_screen(1.0, t)], stroke);
        }
        painter.rect_stroke(square, 0.0, Stroke::new(1.0, theme::BORDER_STRONG), egui::StrokeKind::Middle);

        // Rasterized layout: request the visible window, show the latest one we have.
        let ppp = ctx.pixels_per_point();
        let size = [(canvas.width() * ppp).round().clamp(1.0, 4096.0) as usize, (canvas.height() * ppp).round().clamp(1.0, 4096.0) as usize];
        let key: RasterKey = (targets.clone(), [window.min[0], window.min[1], window.max[0], window.max[1]].map(f32::to_bits), size);
        if let Some((job_key, job_window, rx, job_size)) = &v.job {
            if let Ok(pixels) = rx.try_recv() {
                let image = egui::ColorImage::from_rgba_unmultiplied(*job_size, &pixels);
                let tex = ctx.load_texture("uv_layout", image, TextureOptions::NEAREST);
                v.raster = Some((job_key.clone(), *job_window, tex));
                v.job = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        if v.job.is_none() && v.raster.as_ref().is_none_or(|r| r.0 != key) {
            let (tx, rx) = channel();
            let data = Arc::clone(&data);
            let ids = targets.clone();
            let repaint = ctx.clone();
            std::thread::spawn(move || {
                let meshes: Vec<&uv::UvMesh> = ids.iter().filter_map(|&i| data.meshes[i].as_ref()).collect();
                let _ = tx.send(uv::rasterize(&meshes, window, size));
                repaint.request_repaint();
            });
            v.job = Some((key, window, rx, size));
        }
        if let Some((_, w, tex)) = &v.raster {
            let at = Rect::from_two_pos(to_screen(w.min[0], w.min[1]), to_screen(w.max[0], w.max[1]));
            painter.image(tex.id(), at, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
        }
        if targets.is_empty() {
            painter.text(canvas.center(), Align2::CENTER_CENTER, tr("No UVs on these objects"), FontId::proportional(13.0), theme::TEXT_DIM);
        }
        if r.hovered() {
            ctx.set_cursor_icon(if r.dragged() { CursorIcon::Grabbing } else { CursorIcon::Grab });
        }
    }

    /// "UV · 3 objects" plus mirrored / outside counts, texture toggle and fit.
    fn uv_header(&mut self, ui: &mut Ui, header: Rect, data: Option<&UvData>, targets: &[usize]) {
        let painter = ui.painter().with_clip_rect(header);
        painter.hline(header.x_range(), header.bottom() - 0.5, Stroke::new(1.0, theme::BORDER));
        let mut job = LayoutJob::default();
        let caps = |c| TextFormat { font_id: theme::mono(11.0), color: c, extra_letter_spacing: 0.9, ..Default::default() };
        job.append("UV", 0.0, caps(theme::TEXT));
        if let Some(data) = data {
            let meshes: Vec<&uv::UvMesh> = targets.iter().filter_map(|&i| data.meshes[i].as_ref()).collect();
            let name = match (self.uv_view.material, targets) {
                (Some(m), _) => self.info.as_ref().map(|i| i.materials[m].name.clone()).unwrap_or_default(),
                (None, [one]) => self.info.as_ref().map(|i| i.objects[*one].name.clone()).unwrap_or_default(),
                _ => trf("{n} objects", &[("n", &targets.len())]),
            };
            job.append(&format!("  ·  {name}"), 0.0, TextFormat { font_id: FontId::proportional(12.0), color: theme::TEXT_DIM, ..Default::default() });
            let flipped: usize = meshes.iter().map(|m| m.flipped.len()).sum();
            let outside: usize = meshes.iter().map(|m| m.outside).sum();
            let small = |c| TextFormat { font_id: theme::mono(10.5), color: c, ..Default::default() };
            job.append(&format!("   {} ", tr("Mirrored")), 0.0, small(theme::TEXT_DIM));
            job.append(&thousands(flipped), 0.0, small(if flipped > 0 { theme::ERROR } else { theme::TEXT }));
            job.append(&format!("   {} ", tr("Outside 0–1")), 0.0, small(theme::TEXT_DIM));
            job.append(&thousands(outside), 0.0, small(theme::TEXT));
        }

        let mut child = ui.new_child(UiBuilder::new().max_rect(header.shrink2(vec2(6.0, 4.0))).layout(Layout::right_to_left(Align::Center)));
        child.spacing_mut().item_spacing.x = 2.0;
        if widgets::sized_icon_button(&mut child, icons::close, false, Vec2::splat(theme::TOOLBAR_HEIGHT)).on_hover_text(tr("Close UV view (U)")).clicked() {
            self.uv_view.open = false;
        }
        if widgets::sized_icon_button(&mut child, icons::frame, false, Vec2::splat(theme::TOOLBAR_HEIGHT))
            .on_hover_text(tr("Fit the 0–1 square (double-click)"))
            .clicked()
        {
            self.uv_view.center = [0.5, 0.5];
            self.uv_view.zoom = 1.0;
        }
        self.uv_material_picker(&mut child);
        if text_button(&mut child, "Texture", self.uv_view.show_texture).on_hover_text(tr("Base color texture behind the layout")).clicked() {
            self.uv_view.show_texture = !self.uv_view.show_texture;
        }
        // The text stops where the buttons start, so a narrow pane cuts it instead of hiding
        // it under them.
        let mut text_clip = header;
        text_clip.max.x = child.min_rect().left() - 8.0;
        let galley = painter.layout_job(job);
        painter
            .with_clip_rect(text_clip)
            .galley(pos2(header.left() + 14.0, header.center().y - galley.size().y * 0.5), galley, theme::TEXT);
    }

    /// Texture set dropdown: "Selection" follows the selected objects, a material shows (and
    /// selects) every object that uses it.
    fn uv_material_picker(&mut self, ui: &mut Ui) {
        let Some(info) = &self.info else { return };
        let mut used: Vec<(usize, usize)> = Vec::new();
        for (mi, _) in info.materials.iter().enumerate() {
            let n = info.objects.iter().filter(|o| o.material == mi).count();
            if n > 0 {
                used.push((mi, n));
            }
        }
        if used.len() < 2 {
            return;
        }
        let current = match self.uv_view.material {
            Some(m) => info.materials[m].name.clone(),
            None => tr("Selection").to_string(),
        };
        let mut pick = None;
        egui::ComboBox::from_id_salt("uv_material")
            .selected_text(current)
            .width(170.0)
            .height(480.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(self.uv_view.material.is_none(), tr("Selection")).on_hover_text(tr("UVs of the selected objects")).clicked() {
                    pick = Some(None);
                }
                ui.separator();
                for &(mi, n) in &used {
                    let label = format!("{}  ({n})", info.materials[mi].name);
                    if ui.selectable_label(self.uv_view.material == Some(mi), label).clicked() {
                        pick = Some(Some(mi));
                    }
                }
            })
            .response
            .on_hover_text(tr("Texture set: every object using one material"));
        match pick {
            Some(Some(m)) => self.uv_show_material(m),
            Some(None) => self.uv_view.material = None,
            None => {}
        }
    }

    /// Shows material `m`'s texture set and selects the objects that use it.
    pub(super) fn uv_show_material(&mut self, m: usize) {
        let Some(info) = &self.info else { return };
        if m >= info.materials.len() {
            return;
        }
        let objects: Vec<usize> = (0..info.objects.len()).filter(|&i| info.objects[i].material == m).collect();
        self.selection.selected.fill(false);
        for &i in &objects {
            self.selection.selected[i] = true;
        }
        self.selection.active = objects.first().copied();
        self.uv_view.material = Some(m);
        self.uv_view.material_selection = self.selection.selected.clone();
    }
}
