//! A/B comparison: a second model on its own renderer, drawn side by side or behind a
//! draggable split, with one shared camera (so both always show the same view) and the same
//! shading, overlays and section. The Info tab gets an A/B table (triangles, vertices, mesh
//! check...). Tools (select, measure, section drag) work on model A.

use eframe::egui::{CursorIcon, Id};

use super::*;
use crate::i18n::thousands;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CompareMode {
    SideBySide,
    /// One view, A left of the divider and B right of it.
    Split,
}

pub(super) struct Compare {
    pub(super) renderer: Renderer,
    pub(super) info: SceneInfo,
    qa_rx: Option<Receiver<QaResult>>,
    pub(super) mode: CompareMode,
    /// Divider position across the view in Split mode, 0..1.
    pub(super) split: f32,
    /// Soft transition / gradient width across the divider in Split mode.
    pub(super) gradient: f32,
    /// B's own animation, kept at A's clip and time.
    pub(super) anim: Option<AnimPlayer>,
    /// Comparing different shading styles on the same model.
    pub(super) is_same_model: bool,
    pub(super) style_b: ShadingMode,
    pub(super) wire_overlay_b: bool,
    pub(super) matcap_b: usize,
}

impl ViewerApp {
    pub(super) fn compare_dialog(&mut self, ctx: &egui::Context) {
        let picked = rfd::FileDialog::new()
            .set_title(tr("Compare with…"))
            .add_filter(tr("3D models"), loader::SUPPORTED_EXTENSIONS)
            .pick_file();
        if let Some(path) = picked {
            self.open_compare(path, ctx);
        }
    }

    /// Opens style comparison on the current model.
    pub(super) fn open_compare_same_model(&mut self, ctx: &egui::Context) {
        let Some(info) = &self.info else { return };
        self.open_compare(info.path.clone(), ctx);
    }

    /// Loads `path` as model B.
    pub(super) fn open_compare(&mut self, path: PathBuf, ctx: &egui::Context) {
        if !loader::is_supported(&path) || loader::is_environment(&path) {
            self.open(path, ctx);
            return;
        }
        let (tx, rx) = channel();
        let repaint = ctx.clone();
        let thread_path = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(loader::load_with(&thread_path, &[]));
            repaint.request_repaint();
        });
        self.compare_loading = Some(Loading { path, rx, started: Instant::now() });
    }

    pub(super) fn poll_compare(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if let Some(c) = &mut self.compare {
            if let Some(Ok(result)) = c.qa_rx.as_ref().map(|rx| rx.try_recv()) {
                c.qa_rx = None;
                c.renderer.set_markers(&result.markers);
                c.info.qa = Some(result.report);
            }
        }
        let Some(loading) = &self.compare_loading else { return };
        let Ok(result) = loading.rx.try_recv() else { return };
        let loading = self.compare_loading.take().expect("checked above");
        let result = result.and_then(|s| self.renderer.as_ref().map_or(Ok(()), |r| r.check_fits(&s)).map(|_| s));
        let mut scene = match result {
            Ok(scene) => scene,
            Err(err) => {
                let text = trf("Couldn't open {name}: {error}", &[("name", &file_name(&loading.path)), ("error", &err)]);
                self.show_toast(ctx, text, true);
                return;
            }
        };
        let Some(rs) = frame.wgpu_render_state() else { return };
        let mut renderer = Renderer::new(&rs.device, &rs.queue);
        renderer.upload_scene(&scene);
        if let Some(env) = &self.env_image {
            renderer.set_environment(env);
        }
        let info = scene_info(&scene, &loading.path, loading.started.elapsed());
        let has_morphs = scene.meshes.iter().any(|m| !m.rig.morph_targets.is_empty());
        let animation = std::mem::take(&mut scene.animation);
        let anim = (animation.is_animated() || has_morphs).then(|| AnimPlayer::new(animation, &scene.meshes));
        let is_same_model = self.info.as_ref().is_some_and(|i| i.path == loading.path);
        let (mode, split, gradient) = self.compare.as_ref().map_or(
            if is_same_model { (CompareMode::Split, 0.5, 0.08) } else { (CompareMode::SideBySide, 0.5, 0.0) },
            |c| (c.mode, c.split, c.gradient),
        );
        // Frame both models: same camera, and clip planes that fit the bigger one.
        let mut both = self.scene_bounds();
        for o in &info.objects {
            both.union(&o.bounds);
        }
        self.camera.scene_radius = self.camera.scene_radius.max(both.radius());
        self.camera.frame(&both, true);
        self.compare = Some(Compare {
            renderer,
            info,
            qa_rx: Some(spawn_analysis(scene, ctx)),
            mode,
            split,
            gradient,
            anim,
            is_same_model,
            style_b: ShadingMode::Wireframe,
            wire_overlay_b: true,
            matcap_b: 0,
        });
    }

    /// Poses B like A: same clip (matched by name, else by index) at the same time.
    pub(super) fn sync_compare_animation(&mut self) {
        let (Some(a), Some(c)) = (&self.anim, &mut self.compare) else { return };
        let Some(b) = &mut c.anim else { return };
        let a_clip = a.clip.and_then(|i| a.data.clips.get(i));
        b.clip = a_clip
            .and_then(|clip| b.data.clips.iter().position(|x| x.name == clip.name))
            .or(a.clip.filter(|&i| i < b.data.clips.len()))
            .or(b.clip);
        b.time = a.time.min(b.duration());
        let pose = b.pose();
        c.renderer.set_object_transforms(&pose.object_transforms);
        c.renderer.set_joints(&pose.joints);
        for (i, positions, normals) in &pose.morphed {
            c.renderer.update_mesh_geometry(*i, positions, normals);
        }
    }

    /// Model A's part of the viewport.
    pub(super) fn compare_rect_a(&self, rect: Rect) -> Rect {
        match &self.compare {
            Some(c) if c.mode == CompareMode::SideBySide => {
                Rect::from_min_max(rect.min, pos2((rect.center().x - 1.0).round(), rect.max.y))
            }
            _ => rect,
        }
    }

    /// Renders model B with the shared camera and paints it into its part of the view.
    pub(super) fn render_compare(&mut self, ui: &mut Ui, frame: &mut eframe::Frame, rect: Rect) {
        let effective = self.effective_settings();
        let section = self.section_plane();
        let normal_length = self.normal_length();
        let (grid_cell, grid_fade, grid_axis) = grid_params(&self.camera);
        let print_scale = self.print_scale();
        let (Some(c), Some(rs)) = (&mut self.compare, frame.wgpu_render_state()) else { return };
        let mut settings_b = effective.clone();
        if c.is_same_model {
            settings_b.shading = c.style_b;
            settings_b.show_wire_overlay = c.wire_overlay_b;
            if c.style_b == ShadingMode::Solid {
                settings_b.matcap = c.matcap_b;
            }
        }
        let scene = None;
        let target = match c.mode {
            CompareMode::SideBySide => Rect::from_min_max(pos2((rect.center().x + 1.0).round(), rect.min.y), rect.max),
            CompareMode::Split => rect,
        };
        let ppp = ui.ctx().pixels_per_point();
        let size = [(target.width() * ppp).round().max(1.0) as u32, (target.height() * ppp).round().max(1.0) as u32];
        let input = FrameInput {
            view: self.camera.view_matrix(),
            proj: self.camera.projection(size[0] as f32 / size[1] as f32),
            eye: self.camera.eye(),
            cam_back: self.camera.back(),
            ortho: self.camera.ortho,
            grid_cell,
            grid_fade,
            grid_axis,
            settings: &settings_b,
            pick: None,
            transparent: false,
            section,
            normal_length,
            print_scale,
            scene,
        };
        let texture = {
            let mut egui_renderer = rs.renderer.write();
            c.renderer.render(Some(&mut egui_renderer), size, &input)
        };
        let Some(texture) = texture else { return };
        let painter = ui.painter();
        match c.mode {
            CompareMode::SideBySide => {
                painter.image(texture, target, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                painter.vline(rect.center().x, rect.y_range(), Stroke::new(4.0, Color32::from_black_alpha(160)));
                painter.vline(rect.center().x, rect.y_range(), Stroke::new(1.5, theme::BORDER));
            }
            CompareMode::Split => {
                let x = rect.left() + rect.width() * c.split;
                if c.gradient <= 0.001 {
                    let part = Rect::from_min_max(pos2(x, rect.top()), rect.max);
                    painter.image(texture, part, Rect::from_min_max(pos2(c.split, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                } else {
                    let g = c.gradient.clamp(0.005, 0.45);
                    let t_min = (c.split - g).clamp(0.0, 1.0);
                    let t_max = (c.split + g).clamp(0.0, 1.0);
                    let x_min = rect.left() + rect.width() * t_min;
                    let x_max = rect.left() + rect.width() * t_max;

                    // Transition band with gradient alpha
                    if x_max > x_min {
                        let mut mesh = egui::Mesh::with_texture(texture);
                        let y0 = rect.top();
                        let y1 = rect.bottom();
                        let c_left = Color32::from_white_alpha(0);
                        let c_right = Color32::WHITE;
                        mesh.vertices.push(egui::epaint::Vertex { pos: pos2(x_min, y0), uv: pos2(t_min, 0.0), color: c_left });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pos2(x_max, y0), uv: pos2(t_max, 0.0), color: c_right });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pos2(x_max, y1), uv: pos2(t_max, 1.0), color: c_right });
                        mesh.vertices.push(egui::epaint::Vertex { pos: pos2(x_min, y1), uv: pos2(t_min, 1.0), color: c_left });
                        mesh.indices.extend([0, 1, 2, 0, 2, 3]);
                        painter.add(egui::Shape::mesh(mesh));
                    }

                    // Fully opaque part to the right of x_max
                    if rect.right() > x_max {
                        let right_rect = Rect::from_min_max(pos2(x_max, rect.top()), rect.max);
                        painter.image(texture, right_rect, Rect::from_min_max(pos2(t_max, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                    }
                }
            }
        }
    }

    /// Name chips for A and B, and the Split divider handle.
    pub(super) fn compare_controls(&mut self, ui: &mut Ui, rect: Rect) {
        let Some(c) = &mut self.compare else { return };
        let a_name = if c.is_same_model {
            let label = match self.settings.shading {
                ShadingMode::Wireframe => tr("Wireframe"),
                ShadingMode::Solid => tr("Solid"),
                ShadingMode::Rendered => tr("Rendered"),
            };
            let tris = self.info.as_ref().map(|i| i.triangles).unwrap_or(0);
            (format!("A: {}", label), tris)
        } else {
            self.info.as_ref().map(|i| (i.file_name.clone(), i.triangles)).unwrap_or_default()
        };
        let b_name = if c.is_same_model {
            let label = match (c.style_b, c.wire_overlay_b) {
                (ShadingMode::Wireframe, _) => tr("Wireframe"),
                (ShadingMode::Solid, true) => tr("Solid + Wire"),
                (ShadingMode::Solid, false) => tr("Solid"),
                (ShadingMode::Rendered, _) => tr("Rendered"),
            };
            let tris = self.info.as_ref().map(|i| i.triangles).unwrap_or(0);
            (format!("B: {}", label), tris)
        } else {
            (c.info.file_name.clone(), c.info.triangles)
        };
        let bottom = rect.bottom() - if self.section.is_some() { 64.0 } else { 16.0 };
        let (mode, split) = (c.mode, c.split);
        match mode {
            CompareMode::SideBySide => {
                let a = self.compare_rect_a(rect);
                name_chip(ui, "A", &a_name, pos2(a.center().x, bottom), Align2::CENTER_BOTTOM);
                name_chip(ui, "B", &b_name, pos2(rect.center().x + (rect.right() - rect.center().x) * 0.5, bottom), Align2::CENTER_BOTTOM);
            }
            CompareMode::Split => {
                let x = rect.left() + rect.width() * split;
                name_chip(ui, "A", &a_name, pos2(x - 14.0, bottom), Align2::RIGHT_BOTTOM);
                name_chip(ui, "B", &b_name, pos2(x + 14.0, bottom), Align2::LEFT_BOTTOM);
                // Divider: drag it to wipe between the two models.
                let hit = Rect::from_center_size(pos2(x, rect.center().y), vec2(14.0, rect.height()));
                let r = ui.interact(hit, Id::new("compare_divider"), Sense::drag());
                if r.hovered() || r.dragged() {
                    ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
                }
                if r.dragged() {
                    if let Some(p) = r.interact_pointer_pos() {
                        let t = ((p.x - rect.left()) / rect.width()).clamp(0.02, 0.98);
                        c.split = t;
                    }
                }
                let painter = ui.painter();
                if c.gradient > 0.001 {
                    let g = c.gradient.clamp(0.005, 0.45);
                    let x_min = rect.left() + rect.width() * (c.split - g).clamp(0.0, 1.0);
                    let x_max = rect.left() + rect.width() * (c.split + g).clamp(0.0, 1.0);
                    let stroke_fade = Stroke::new(1.0, Color32::from_white_alpha(50));
                    painter.extend(egui::Shape::dashed_line(&[pos2(x_min, rect.top()), pos2(x_min, rect.bottom())], stroke_fade, 4.0, 4.0));
                    painter.extend(egui::Shape::dashed_line(&[pos2(x_max, rect.top()), pos2(x_max, rect.bottom())], stroke_fade, 4.0, 4.0));
                }
                painter.vline(x, rect.y_range(), Stroke::new(3.0, Color32::from_black_alpha(140)));
                painter.vline(x, rect.y_range(), Stroke::new(1.5, theme::TEXT));
                let knob = pos2(x, rect.center().y);
                painter.circle(knob, 13.0, theme::SURFACE, Stroke::new(1.5, theme::TEXT));
                for dir in [-1.0f32, 1.0] {
                    let tip = knob + vec2(dir * 7.0, 0.0);
                    let back = knob + vec2(dir * 2.5, 0.0);
                    painter.add(egui::Shape::convex_polygon(
                        vec![tip, back + vec2(0.0, -4.0), back + vec2(0.0, 4.0)],
                        theme::TEXT,
                        Stroke::NONE,
                    ));
                }
            }
        }
    }



    pub(super) fn apply_compare_action(&mut self, action: CompareAction, ctx: &egui::Context) {
        match action {
            CompareAction::None => {}
            CompareAction::Open => self.compare_dialog(ctx),
            CompareAction::OpenSameModel => self.open_compare_same_model(ctx),
            CompareAction::Mode(m) => {
                if let Some(c) = &mut self.compare {
                    c.mode = m;
                }
            }
            CompareAction::Swap => {
                let (Some(a), Some(c)) = (&self.info, &self.compare) else { return };
                let (a, b) = (a.path.clone(), c.info.path.clone());
                self.open(b, ctx);
                self.open_compare(a, ctx);
            }
            CompareAction::Close => {
                self.compare = None;
                self.compare_loading = None;
            }
        }
    }

    /// A/B table in the Info tab.
    pub(super) fn compare_table(&self, ui: &mut Ui) {
        let (Some(a), Some(c)) = (&self.info, &self.compare) else { return };
        let b = &c.info;
        widgets::section(ui, "Comparison");
        let size = |i: &SceneInfo| {
            let mut bb = Aabb::EMPTY;
            for o in &i.objects {
                bb.union(&o.bounds);
            }
            bb.size()
        };
        let (sa, sb) = (size(a), size(b));
        let qa = |i: &SceneInfo, f: fn(&qa::Report) -> usize| i.qa.as_ref().map(f);
        let rows: [(&str, Option<usize>, Option<usize>); 9] = [
            ("Triangles", Some(a.triangles), Some(b.triangles)),
            ("Vertices", Some(a.vertices), Some(b.vertices)),
            ("Objects", Some(a.objects.len()), Some(b.objects.len())),
            ("Materials", Some(a.materials.len()), Some(b.materials.len())),
            ("Textures", Some(a.images.len()), Some(b.images.len())),
            ("Non-manifold", qa(a, |r| r.non_manifold_edges), qa(b, |r| r.non_manifold_edges)),
            ("Open edges", qa(a, |r| r.open_edges), qa(b, |r| r.open_edges)),
            ("Overlapping", qa(a, |r| r.overlapping_vertices), qa(b, |r| r.overlapping_vertices)),
            ("Inverted normals", qa(a, |r| r.inverted_normals), qa(b, |r| r.inverted_normals)),
        ];
        let value = |v: Option<usize>| v.map_or("…".to_string(), thousands);
        let delta = |va: Option<usize>, vb: Option<usize>| match (va, vb) {
            (Some(x), Some(y)) if x == y => "=".to_string(),
            (Some(0), Some(_)) => "+".to_string(),
            (Some(x), Some(y)) => crate::i18n::decimal(format!("{:+.0}%", (y as f64 - x as f64) / x as f64 * 100.0)),
            _ => String::new(),
        };
        let mut lines: Vec<(String, String, String, String)> = vec![(String::new(), "A".into(), "B".into(), "Δ".into())];
        for (label, va, vb) in rows {
            lines.push((tr(label).to_string(), value(va), value(vb), delta(va, vb)));
        }
        for (d, axis) in crate::axes::NAMES.into_iter().enumerate() {
            let k = self.settings.up_axis.internal(d).0;
            lines.push((trf("Size {axis}", &[("axis", &axis)]), fmt_len(sa[k]), fmt_len(sb[k]), String::new()));
        }
        // Fixed columns: label, A and B right-aligned, delta right-aligned at the edge.
        Frame::new()
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(CornerRadius::same(theme::RADIUS))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let width = ui.available_width();
                for (k, (label, va, vb, d)) in lines.iter().enumerate() {
                    let (rect, _) = ui.allocate_exact_size(vec2(width, if k == 0 { 26.0 } else { 28.0 }), Sense::hover());
                    if k > 0 {
                        ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme::BORDER));
                    }
                    let y = rect.center().y;
                    let col_a = rect.left() + width * 0.6;
                    let col_b = rect.left() + width * 0.8;
                    let (font, color) = if k == 0 { (theme::mono(10.0), theme::TEXT_FAINT) } else { (theme::mono(11.0), theme::TEXT) };
                    let painter = ui.painter();
                    let clip = Rect::from_min_max(rect.min, pos2(rect.left() + width * 0.38, rect.max.y));
                    painter.with_clip_rect(clip).text(pos2(rect.left() + 12.0, y), Align2::LEFT_CENTER, label, FontId::proportional(11.5), theme::TEXT_DIM);
                    painter.text(pos2(col_a, y), Align2::RIGHT_CENTER, va, font.clone(), color);
                    painter.text(pos2(col_b, y), Align2::RIGHT_CENTER, vb, font.clone(), color);
                    painter.text(pos2(rect.right() - 10.0, y), Align2::RIGHT_CENTER, d, font, theme::TEXT_DIM);
                }
            });
        ui.add_space(12.0);
    }
}

pub(super) enum CompareAction {
    None,
    Open,
    OpenSameModel,
    Mode(CompareMode),
    Swap,
    Close,
}

/// "A  model.glb · 12.345 tris" chip.
fn name_chip(ui: &Ui, badge: &str, (name, tris): &(String, usize), at: Pos2, align: Align2) {
    let painter = ui.painter();
    let text = painter.layout_no_wrap(name.clone(), FontId::proportional(13.0), theme::TEXT);
    let detail = painter.layout_no_wrap(trf("{tris} tris", &[("tris", &thousands(*tris))]), theme::mono(10.5), theme::TEXT_DIM);
    let badge_size = vec2(20.0, 20.0);
    let size = vec2(8.0 + badge_size.x + 8.0 + text.size().x + 10.0 + detail.size().x + 12.0, 32.0);
    let rect = align.anchor_size(at, size);
    painter.rect_filled(rect, CornerRadius::same(theme::RADIUS), Color32::from_black_alpha(215));
    painter.rect_stroke(rect, CornerRadius::same(theme::RADIUS), Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
    let b = Rect::from_min_size(pos2(rect.left() + 8.0, rect.center().y - badge_size.y * 0.5), badge_size);
    painter.rect_filled(b, CornerRadius::same(3), theme::TEXT);
    painter.text(b.center(), Align2::CENTER_CENTER, badge, theme::bold(12.0), theme::ON_ACCENT);
    let x = b.right() + 8.0;
    let tw = text.size().x;
    painter.galley(pos2(x, rect.center().y - text.size().y * 0.5), text, theme::TEXT);
    painter.galley(pos2(x + tw + 10.0, rect.center().y - detail.size().y * 0.5), detail, theme::TEXT_DIM);
}
