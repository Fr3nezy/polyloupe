//! Window chrome in the PolyLoupe style: custom title bar with menus, tool rail, viewport
//! toolbar and HUD, the inspector (Info / Materials / Scene), footer, empty state and banners.
//! Tokens and controls come from `ui::theme` and `ui::widgets`.

use eframe::egui::{
    CursorIcon, Id, Image, Order, PointerButton, ResizeDirection, ScrollArea, TextFormat, Vec2, ViewportCommand,
    text::LayoutJob,
};

use super::*;
use crate::i18n::thousands;
use crate::settings::RenderResolution;
use crate::ui::widgets::{dashed_rect, primary_button, stat_card, table, tag, text_button, tool_button, toolbar, toolbar_separator};

pub(super) const TITLE_HEIGHT: f32 = 44.0;
pub(super) const FOOTER_HEIGHT: f32 = 32.0;
pub(super) const RAIL_WIDTH: f32 = 52.0;
pub(super) const INSPECTOR_WIDTH: f32 = 360.0;
const RESIZE_MARGIN: f32 = 5.0;

/// Left-drag behavior chosen in the tool rail (the navigation preset's own mouse keys always work).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tool {
    Select,
    Orbit,
    Pan,
    Zoom,
    /// Click two surface points to measure the distance between them.
    Measure,
    /// Drag to slide the section plane.
    Section,
    /// Gizmos to move, rotate and scale objects (Manufacturing), with lay on face and export.
    Move,
    Rotate,
    Scale,
}

impl Tool {
    /// One of the gizmo tools.
    pub(super) fn transforms(self) -> bool {
        matches!(self, Tool::Move | Tool::Rotate | Tool::Scale)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum InspectorTab {
    Info,
    Shading,
    Render,
    Materials,
    Scene,
}

/// What a banner's button does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastAction {
    /// Opens the inspector on the Materials tab (missing textures).
    ShowMaterials,
    /// Opens the releases page (a newer version exists).
    OpenReleases,
}

impl ViewerApp {
    // --- Title bar -----------------------------------------------------------------------------

    pub(super) fn title_bar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let bar = ui.max_rect();
        // The empty part of the bar moves the window; a double click maximizes, like Windows.
        let drag = ui.interact(bar, Id::new("title_drag"), Sense::click_and_drag());
        if drag.drag_started_by(PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }

        // File name and format, centered on the whole window.
        if let Some(info) = &self.info {
            let name = ui.painter().layout_no_wrap(info.file_name.clone(), theme::medium(13.0), theme::TEXT);
            let format = info.path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
            let tag_job = ui.painter().layout_job(theme::caps(&format, 10.0, theme::TEXT_DIM));
            let width = name.size().x + 10.0 + tag_job.size().x + 12.0;
            let x = bar.center().x - width * 0.5;
            let y = bar.center().y;
            ui.painter().galley(pos2(x, y - name.size().y * 0.5), name.clone(), theme::TEXT);
            let tag_rect = Rect::from_min_size(
                pos2(x + name.size().x + 10.0, y - tag_job.size().y * 0.5 - 2.0),
                tag_job.size() + vec2(12.0, 4.0),
            );
            ui.painter().rect_stroke(tag_rect, CornerRadius::same(2), Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
            ui.painter().galley(tag_rect.center() - tag_job.size() * 0.5, tag_job, theme::TEXT_DIM);
        }

        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if let Some(logo) = &self.logo {
                ui.add(Image::new(logo).fit_to_exact_size(vec2(22.0, 22.0)).corner_radius(5));
            }
            let mut word = LayoutJob::default();
            let format = |font| TextFormat { font_id: font, color: theme::TEXT, extra_letter_spacing: -0.3, ..Default::default() };
            word.append("Poly", 0.0, format(FontId::proportional(15.0)));
            word.append("Loupe", 0.0, format(theme::bold(15.0)));
            ui.label(word);
            ui.add_space(6.0);
            ui.spacing_mut().item_spacing.x = 2.0;
            self.menus(ui);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let size = vec2(44.0, 32.0);
                let close = window_button(ui, icons::close, size, true).on_hover_text(tr("Close"));
                if close.clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                let icon: icons::IconFn = if maximized { icons::window_restore } else { icons::window_maximize };
                if window_button(ui, icon, size, false).clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
                }
                if window_button(ui, icons::window_minimize, size, false).clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
                }
                // Side panel switch, always in the same place at the right edge.
                ui.add_space(10.0);
                let open = self.settings.show_sidebar;
                let panel = widgets::sized_icon_button(ui, icons::panel, open, vec2(36.0, 30.0))
                    .on_hover_text(tr(if open { "Hide the side panel (N)" } else { "Show the side panel (N)" }));
                if panel.clicked() {
                    self.settings.show_sidebar = !open;
                }
            });
        });
    }

    /// Borderless window: resize from the edges and corners.
    pub(super) fn window_edges(&self, ctx: &egui::Context) {
        let (maximized, fullscreen) = ctx.input(|i| {
            (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false))
        });
        if maximized || fullscreen || self.capture.is_some() {
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let r = ctx.content_rect();
        let (w, e) = (pos.x - r.left() < RESIZE_MARGIN, r.right() - pos.x < RESIZE_MARGIN);
        let (n, s) = (pos.y - r.top() < RESIZE_MARGIN, r.bottom() - pos.y < RESIZE_MARGIN);
        let (dir, cursor) = match (n, s, w, e) {
            (true, _, true, _) => (ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest),
            (true, _, _, true) => (ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast),
            (_, true, true, _) => (ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest),
            (_, true, _, true) => (ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast),
            (true, ..) => (ResizeDirection::North, CursorIcon::ResizeNorth),
            (_, true, ..) => (ResizeDirection::South, CursorIcon::ResizeSouth),
            (_, _, true, _) => (ResizeDirection::West, CursorIcon::ResizeWest),
            (_, _, _, true) => (ResizeDirection::East, CursorIcon::ResizeEast),
            _ => return,
        };
        ctx.set_cursor_icon(cursor);
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }

    fn menus(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.style_mut().spacing.button_padding = vec2(10.0, 7.0);
            ui.menu_button(tr("File"), |ui| {
                if ui.add(egui::Button::new(tr("Open…")).shortcut_text("Ctrl O")).clicked() {
                    ui.close();
                    self.open_dialog(&ctx);
                }
                ui.add_enabled_ui(self.info.is_some(), |ui| {
                    if ui.add(egui::Button::new(tr("Compare with…")).shortcut_text("Ctrl Shift O")).clicked() {
                        ui.close();
                        self.compare_dialog(&ctx);
                    }
                });
                let recent = self.settings.recent_files.clone();
                ui.add_enabled_ui(!recent.is_empty(), |ui| {
                    ui.menu_button(tr("Open Recent"), |ui| {
                        for path in &recent {
                            let r = ui.button(file_name(Path::new(path))).on_hover_text(path);
                            if r.clicked() {
                                ui.close();
                                self.open(PathBuf::from(path), &ctx);
                            }
                        }
                        ui.separator();
                        if ui.button(tr("Clear Recent Files")).clicked() {
                            self.settings.recent_files.clear();
                            ui.close();
                        }
                    });
                });
                if ui.button(tr("Load HDRI Environment…")).clicked() {
                    ui.close();
                    self.open_hdri_dialog();
                    self.settings.shading = ShadingMode::Rendered;
                }
                ui.add_enabled_ui(self.info.is_some(), |ui| {
                    if ui.add(egui::Button::new(tr("Export Image…")).shortcut_text("F12")).clicked() {
                        ui.close();
                        self.export_image(&ctx);
                    }
                    if ui.button(tr("Export Turntable…")).clicked() {
                        ui.close();
                        self.show_turntable = true;
                    }
                    if self.manufacturing()
                        && ui.button(tr("Export Model…")).on_hover_text(tr("Save the model as STL or 3MF, with the Move tool's changes")).clicked()
                    {
                        ui.close();
                        self.export_model_dialog(&ctx);
                    }
                });
                ui.separator();
                if ui.add(egui::Button::new(tr("Quit")).shortcut_text("Ctrl Q")).clicked() {
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            });
            ui.menu_button(tr("Edit"), |ui| {
                if ui.add(egui::Button::new(tr("Preferences…")).shortcut_text("Ctrl ,")).clicked() {
                    self.show_prefs = true;
                    ui.close();
                }
            });
            ui.menu_button(tr("View"), |ui| {
                if ui.add(egui::Button::new(tr("Inspector")).shortcut_text("N").selected(self.settings.show_sidebar)).clicked() {
                    self.settings.show_sidebar = !self.settings.show_sidebar;
                    ui.close();
                }
                ui.separator();
                if ui.add(egui::Button::new(tr("Frame All")).shortcut_text("Home")).clicked() {
                    self.frame_all();
                    ui.close();
                }
                if ui.add(egui::Button::new(tr("Frame Selected")).shortcut_text(".")).clicked() {
                    self.frame_selected();
                    ui.close();
                }
                ui.separator();
                for (view, key) in [
                    (AxisView::Front, "1"),
                    (AxisView::Back, "Ctrl 1"),
                    (AxisView::Right, "3"),
                    (AxisView::Left, "Ctrl 3"),
                    (AxisView::Top, "7"),
                    (AxisView::Bottom, "Ctrl 7"),
                ] {
                    if ui.add(egui::Button::new(tr(view.label())).shortcut_text(key)).clicked() {
                        self.camera.set_axis_view(view);
                        ui.close();
                    }
                }
                ui.separator();
                let label = tr(if self.camera.ortho { "Perspective" } else { "Orthographic" });
                if ui.add(egui::Button::new(label).shortcut_text("5")).clicked() {
                    self.camera.toggle_ortho();
                    ui.close();
                }
                ui.checkbox(&mut self.camera.auto_perspective, tr("Auto Perspective"));
                ui.separator();
                if ui.add(egui::Button::new(tr("Toggle Fullscreen")).shortcut_text("F11")).clicked() {
                    let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                    ctx.send_viewport_cmd(ViewportCommand::Fullscreen(!fullscreen));
                    ui.close();
                }
            });
            ui.menu_button(tr("Select"), |ui| {
                if ui.add(egui::Button::new(tr("All")).shortcut_text("A")).clicked() {
                    self.selection.all(&self.visible);
                    ui.close();
                }
                if ui.add(egui::Button::new(tr("None")).shortcut_text("Alt A")).clicked() {
                    self.selection.clear();
                    ui.close();
                }
                ui.separator();
                if ui.add(egui::Button::new(tr("Hide Selected")).shortcut_text("H")).clicked() {
                    self.hide_selected(true);
                    ui.close();
                }
                if ui.add(egui::Button::new(tr("Hide Unselected")).shortcut_text("Shift H")).clicked() {
                    self.hide_selected(false);
                    ui.close();
                }
                if ui.add(egui::Button::new(tr("Reveal Hidden")).shortcut_text("Alt H")).clicked() {
                    self.reveal_all();
                    ui.close();
                }
            });
            ui.menu_button(tr("Help"), |ui| self.help_menu(ui));
        });
    }

    fn help_menu(&mut self, ui: &mut Ui) {
        let group = |ui: &mut Ui, title: &str, rows: &[(&str, &str)]| {
            widgets::section(ui, title);
            for &(keys, action) in rows {
                ui.add(egui::Button::new(tr(action)).shortcut_text(key_names(keys)).frame(false));
            }
            ui.separator();
        };
        let mut nav: Vec<(&str, &str)> = self.settings.navigation.help().to_vec();
        nav.extend([
            ("Home", "Frame all"),
            (".", "Frame selected"),
            ("1  3  7  (Ctrl)", "Front / Right / Top (opposite)"),
            ("2  4  6  8", "Orbit in steps"),
            ("5", "Perspective / Orthographic"),
        ]);
        group(ui, "Navigation", &nav);
        group(ui, "Selection", &[
            ("LMB  /  Shift LMB", "Select / extend"),
            ("A  /  Alt A", "Select all / none"),
            ("H  /  Shift H  /  Alt H", "Hide / hide others / reveal"),
            ("N", "Inspector"),
            ("Q  O  G", "Select / orbit / pan tool"),
        ]);
        group(ui, "Animation", &[
            ("Space", "Play / pause"),
            ("←  /  →", "Previous / next frame"),
            ("Shift ←  /  Shift →", "Jump to start / end"),
        ]);
        group(ui, "Shading", &[
            ("Z", "Shading pie menu"),
            ("Shift Z", "Toggle wireframe"),
            ("Alt Z", "Toggle X-ray"),
            ("Shift Alt Z", "Toggle overlays"),
            ("F12", "Export image"),
        ]);
        let open = |url: String| {
            let _ = std::process::Command::new("explorer.exe").arg(url).spawn();
        };
        if ui.button(tr("Report a bug or suggest a feature…")).clicked() {
            open(format!("{REPO_URL}/issues/new/choose"));
            ui.close();
        }
        if ui.button(tr("Check for updates…")).clicked() {
            open(format!("{REPO_URL}/releases/latest"));
            ui.close();
        }
        ui.separator();
        ui.label(theme::caps(&format!("{APP_NAME} v{}", env!("CARGO_PKG_VERSION")), 11.0, theme::TEXT_DIM));
    }

    // --- Tool rail -----------------------------------------------------------------------------

    pub(super) fn tool_rail(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            // Two groups, like Blender's toolbar and its navigation controls: what the left button
            // does to the model, then camera moves.
            let tools: [(Tool, icons::IconFn, &str); 6] = [
                (Tool::Select, icons::cursor, "Select (Q)"),
                (Tool::Move, icons::move_arrows, "Move (W)"),
                (Tool::Rotate, icons::rotate, "Rotate (E)"),
                (Tool::Scale, icons::scale, "Scale (R)"),
                (Tool::Measure, icons::ruler, "Measure (M)"),
                (Tool::Section, icons::section, "Section"),
            ];
            let navigation: [(Tool, icons::IconFn, &str); 3] = [
                (Tool::Orbit, icons::orbit, "Orbit (O)"),
                (Tool::Pan, icons::hand, "Pan (G)"),
                (Tool::Zoom, icons::magnifier, "Zoom (drag up/down)"),
            ];
            for (group, items) in [("Tools", &tools[..]), ("View", &navigation[..])] {
                if group == "View" {
                    ui.add_space(6.0);
                    let (line, _) = ui.allocate_exact_size(vec2(28.0, 1.0), Sense::hover());
                    ui.painter().hline(line.x_range(), line.center().y, Stroke::new(1.0, theme::BORDER));
                    ui.add_space(6.0);
                }
                let (caption, _) = ui.allocate_exact_size(vec2(RAIL_WIDTH - 8.0, 14.0), Sense::hover());
                ui.painter().text(caption.center(), Align2::CENTER_CENTER, tr(group).to_uppercase(), theme::mono(8.5), theme::TEXT_FAINT);
                for &(tool, icon, tip) in items {
                    if tool.transforms() && !self.manufacturing() {
                        continue;
                    }
                    if tool_button(ui, icon, self.tool == tool).on_hover_text(tr(tip)).clicked() {
                        self.tool = tool;
                        if tool == Tool::Section && self.section.is_none() {
                            self.section = Some(Section::default());
                        }
                    }
                }
            }
        });
    }

    // --- Viewport toolbar and HUD ----------------------------------------------------------------

    /// Dedicated comparison headers: separates Window A and Window B cleanly into their own floating toolbars.
    pub(super) fn compare_toolbars(&mut self, ctx: &egui::Context, viewport: Rect) {
        let mut compare_action = super::compare::CompareAction::None;
        let Some(c) = &mut self.compare else { return };
        let mode = c.mode;
        let is_same = c.is_same_model;
        let top = viewport.top() + 12.0;

        // A over the left half, B over the right one, the mode bar in the middle. Each side bar
        // is pushed in from the viewport edge and out from the middle bar (last frame's widths),
        // so the three never overlap however narrow the view gets.
        let width = |id: &str| ctx.memory(|m| m.area_rect(Id::new(id))).map_or(0.0, |r| r.width());
        let (w_a, w_center, w_b) = (width("compare_bar_a"), width("compare_bar_center"), width("compare_bar_b"));
        let gap = 8.0;
        let mid = viewport.center().x;
        let place = |w: f32, wanted: f32, lo: f32, hi: f32| {
            let half = w * 0.5;
            if hi - lo < w { (lo + hi) * 0.5 } else { wanted.clamp(lo + half, hi - half) }
        };
        let x_a = place(w_a, (viewport.left() + mid) * 0.5, viewport.left() + gap, mid - w_center * 0.5 - gap);
        let x_b = place(w_b, (mid + viewport.right()) * 0.5, mid + w_center * 0.5 + gap, viewport.right() - gap);
        let (pos_a, pos_center, pos_b) = (pos2(x_a, top), pos2(mid, top), pos2(x_b, top));

        // 1. Toolbar A (Window A)
        let s = &mut self.settings;
        egui::Area::new(Id::new("compare_bar_a"))
            .order(Order::Middle)
            .fixed_pos(pos_a)
            .pivot(Align2::CENTER_TOP)
            .show(ctx, |ui| {
                toolbar(ui, |ui| {
                    let (badge_r, _) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::hover());
                    ui.painter().rect_filled(badge_r, CornerRadius::same(3), theme::ACCENT);
                    ui.painter().text(badge_r.center(), Align2::CENTER_CENTER, "A", theme::bold(12.0), theme::ON_ACCENT);

                    toolbar_separator(ui);

                    for (m, icon, tip) in [
                        (ShadingMode::Wireframe, icons::wireframe as icons::IconFn, "Wireframe (Shift Z)"),
                        (ShadingMode::Solid, icons::solid, "Solid"),
                        (ShadingMode::Rendered, icons::rendered, "Rendered"),
                    ] {
                        if widgets::sized_icon_button(ui, icon, s.shading == m, Vec2::splat(theme::TOOLBAR_HEIGHT - 4.0))
                            .on_hover_text(tr(tip))
                            .clicked()
                        {
                            s.shading = m;
                        }
                    }

                    toolbar_separator(ui);
                    let r = widgets::sized_icon_button(ui, icons::overlays, s.show_overlays, Vec2::splat(theme::TOOLBAR_HEIGHT - 4.0))
                        .on_hover_text(tr("Show overlays (Shift Alt Z)"));
                    if r.clicked() {
                        s.show_overlays = !s.show_overlays;
                    }
                });
            });

        // 2. Center Toolbar (Mode, Gradient, Close)
        egui::Area::new(Id::new("compare_bar_center"))
            .order(Order::Middle)
            .fixed_pos(pos_center)
            .pivot(Align2::CENTER_TOP)
            .show(ctx, |ui| {
                toolbar(ui, |ui| {
                    if widgets::sized_icon_button(ui, icons::compare_side, mode == super::compare::CompareMode::SideBySide, Vec2::splat(theme::TOOLBAR_HEIGHT))
                        .on_hover_text(tr("A/B side by side"))
                        .clicked()
                    {
                        compare_action = super::compare::CompareAction::Mode(super::compare::CompareMode::SideBySide);
                    }
                    if widgets::sized_icon_button(ui, icons::compare_split, mode == super::compare::CompareMode::Split, Vec2::splat(theme::TOOLBAR_HEIGHT))
                        .on_hover_text(tr("A/B split: drag the divider"))
                        .clicked()
                    {
                        compare_action = super::compare::CompareAction::Mode(super::compare::CompareMode::Split);
                    }

                    if mode == super::compare::CompareMode::Split {
                        toolbar_separator(ui);
                        ui.label(theme::caps(tr("Gradient"), 10.0, theme::TEXT_DIM));
                        ui.spacing_mut().slider_width = 90.0;
                        ui.add(egui::Slider::new(&mut c.gradient, 0.0..=0.35).show_value(false))
                            .on_hover_text(tr("Gradient"));
                    }

                    toolbar_separator(ui);
                    if widgets::sized_icon_button(ui, icons::close, false, Vec2::splat(theme::TOOLBAR_HEIGHT))
                        .on_hover_text(tr("Close the comparison"))
                        .clicked()
                    {
                        compare_action = super::compare::CompareAction::Close;
                    }
                });
            });

        // 3. Toolbar B (Window B)
        egui::Area::new(Id::new("compare_bar_b"))
            .order(Order::Middle)
            .fixed_pos(pos_b)
            .pivot(Align2::CENTER_TOP)
            .show(ctx, |ui| {
                toolbar(ui, |ui| {
                    let (badge_r, _) = ui.allocate_exact_size(vec2(22.0, 20.0), Sense::hover());
                    ui.painter().rect_filled(badge_r, CornerRadius::same(3), theme::TEXT);
                    ui.painter().text(badge_r.center(), Align2::CENTER_CENTER, "B", theme::bold(12.0), theme::BG_APP);

                    toolbar_separator(ui);

                    if is_same {
                        let cur_style = c.style_b;
                        let cur_wire = c.wire_overlay_b;
                        for (sb, wb, icon, tip) in [
                            (ShadingMode::Wireframe, false, icons::wireframe as icons::IconFn, "Style B: Wireframe"),
                            (ShadingMode::Solid, true, icons::sliders, "Style B: Solid + Wireframe"),
                            (ShadingMode::Solid, false, icons::solid, "Style B: Solid"),
                            (ShadingMode::Rendered, false, icons::rendered, "Style B: Rendered"),
                        ] {
                            let active = cur_style == sb && cur_wire == wb;
                            if widgets::sized_icon_button(ui, icon, active, Vec2::splat(theme::TOOLBAR_HEIGHT - 4.0))
                                .on_hover_text(tr(tip))
                                .clicked()
                            {
                                c.style_b = sb;
                                c.wire_overlay_b = wb;
                            }
                        }
                    } else {
                        if text_button(ui, "A⇄B", false).on_hover_text(tr("Swap A and B")).clicked() {
                            compare_action = super::compare::CompareAction::Swap;
                        }
                    }
                });
            });

        self.toolbar_rect = Rect::from_min_size(pos2(viewport.left(), top), vec2(viewport.width(), theme::TOOLBAR_HEIGHT + 8.0));
        self.apply_compare_action(compare_action, ctx);
    }

    /// Shading spheres, overlays, X-ray, texture channel, projection, frame and inspector toggles.
    pub(super) fn viewport_toolbar(&mut self, ctx: &egui::Context, viewport: Rect) {
        if self.compare.is_some() {
            self.compare_toolbars(ctx, viewport);
            return;
        }

        let capture_popover = self.capture.as_ref().and_then(|c| c.opts.popover.clone());
        let mut frame = false;
        let mut shading_panel = false;
        let mut compare_action = super::compare::CompareAction::None;
        let half = self.toolbar_rect.width() * 0.5;
        let mut pos = pos2(viewport.center().x, viewport.top() + 12.0);
        pos.x = pos.x.min(viewport.right() - half - 8.0).max(viewport.left() + half + 8.0);
        let area = egui::Area::new(Id::new("viewport_toolbar"))
            .order(Order::Middle)
            .fixed_pos(pos)
            .pivot(Align2::CENTER_TOP)
            .show(ctx, |ui| {
                toolbar(ui, |ui| {
                    // Workspace first: it decides which tools and materials the rest offers.
                    let mut workspace = self.workspace;
                    let options = [
                        (Workspace::Manufacturing, icons::manufacturing as icons::IconFn, Workspace::Manufacturing.label()),
                        (Workspace::Art, icons::art as icons::IconFn, Workspace::Art.label()),
                    ];
                    let tip = tr("Workspace\nManufacturing: CAD and 3D printing, millimeters, plain plastic material\n3D Art: textures, UVs, animation, the file's own materials");
                    let switched = ui.scope(|ui| widgets::segmented_icons(ui, &mut workspace, &options, viewport.width() > 1000.0));
                    switched.response.on_hover_text(tip);
                    if switched.inner {
                        self.set_workspace(workspace);
                    }
                    toolbar_separator(ui);
                    let panel_open = self.settings.show_sidebar && self.inspector_tab == InspectorTab::Shading;
                    let s = &mut self.settings;
                    Frame::new()
                        .fill(theme::BG_APP)
                        .stroke(Stroke::new(1.0, theme::BORDER))
                        .corner_radius(CornerRadius::same(theme::RADIUS))
                        .inner_margin(Margin::same(1))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let size = Vec2::splat(theme::TOOLBAR_HEIGHT - 4.0);
                            for (mode, icon, tip) in [
                                (ShadingMode::Wireframe, icons::wireframe as icons::IconFn, "Wireframe (Shift Z)"),
                                (ShadingMode::Solid, icons::solid, "Solid"),
                                (ShadingMode::Rendered, icons::rendered, "Rendered"),
                            ] {
                                let r = widgets::sized_icon_button(ui, icon, s.shading == mode, size)
                                    .on_hover_text(trf("{mode}  ·  Z for the pie menu", &[("mode", &tr(tip))]));
                                if r.clicked() {
                                    s.shading = mode;
                                }
                            }
                            let (line, _) = ui.allocate_exact_size(vec2(5.0, size.y), Sense::hover());
                            ui.painter().vline(line.center().x, line.y_range().shrink(5.0), Stroke::new(1.0, theme::BORDER));
                            shading_panel = widgets::sized_icon_button(ui, icons::sliders, panel_open, size)
                                .on_hover_text(tr("Shading settings: they change with the mode (panel on the right)"))
                                .clicked();
                        });
                    toolbar_separator(ui);

                    let r = widgets::sized_icon_button(ui, icons::overlays, s.show_overlays, Vec2::splat(theme::TOOLBAR_HEIGHT))
                        .on_hover_text(tr("Show overlays (Shift Alt Z)"));
                    if r.clicked() {
                        s.show_overlays = !s.show_overlays;
                    }
                    let chevron = widgets::sized_icon_button(ui, icons::chevron_down, false, vec2(20.0, theme::TOOLBAR_HEIGHT))
                        .on_hover_text(tr("Overlay options"));
                    if capture_popover.as_deref() == Some("overlays") {
                        egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&chevron));
                    }
                    egui::Popup::from_toggle_button_response(&chevron)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .align(RectAlign::BOTTOM_START)
                        .width(240.0)
                        .show(|ui| popovers::overlays(ui, s));
                    let xray = s.xray();
                    let r = widgets::sized_icon_button(ui, icons::xray, xray, Vec2::splat(theme::TOOLBAR_HEIGHT))
                        .on_hover_text(tr("Toggle X-ray (Alt Z)"));
                    if r.clicked() {
                        s.toggle_xray();
                    }

                    let missing = self.info.as_ref().map_or(0, |i| i.missing.len()).saturating_mul(!self.manufacturing() as usize);
                    if missing > 0 {
                        toolbar_separator(ui);
                        let r = tag(ui, &format!("{missing} !"), true)
                            .interact(Sense::click())
                            .on_hover_text(trf("{n} textures not found", &[("n", &missing)]));
                        if r.clicked() {
                            self.settings.show_sidebar = true;
                            self.inspector_tab = InspectorTab::Materials;
                        }
                    }
                    if self.info.is_some() {
                        toolbar_separator(ui);
                        if !self.manufacturing() && text_button(ui, "UV", self.uv_view.open).on_hover_text(tr("UV layout (U)")).clicked() {
                            self.uv_view.open = !self.uv_view.open;
                        }
                        if widgets::sized_icon_button(ui, icons::compare_split, false, Vec2::splat(theme::TOOLBAR_HEIGHT))
                            .on_hover_text(tr("Split view: compare two shading styles on this model"))
                            .clicked()
                        {
                            compare_action = super::compare::CompareAction::OpenSameModel;
                        }
                        if widgets::sized_icon_button(ui, icons::compare_side, self.compare_loading.is_some(), Vec2::splat(theme::TOOLBAR_HEIGHT))
                            .on_hover_text(tr("Compare with another model (Ctrl Shift O), or drop it on the right half"))
                            .clicked()
                        {
                            compare_action = super::compare::CompareAction::Open;
                        }
                    }
                    toolbar_separator(ui);
                    let projection = if self.camera.ortho { "Orthographic" } else { "Perspective" };
                    if text_button(ui, projection, false).on_hover_text(tr("Perspective / Orthographic (5)")).clicked() {
                        self.camera.toggle_ortho();
                    }
                    if widgets::sized_icon_button(ui, icons::frame, false, Vec2::splat(theme::TOOLBAR_HEIGHT))
                        .on_hover_text(tr("Frame the model (Home)"))
                        .clicked()
                    {
                        frame = true;
                    }
                });
            });
        if area.response.rect != self.toolbar_rect {
            self.toolbar_rect = area.response.rect;
            ctx.request_repaint();
        }
        if shading_panel {
            self.toggle_inspector_tab(InspectorTab::Shading);
        }
        self.apply_compare_action(compare_action, ctx);
        if frame {
            self.frame_all();
        }
    }

    /// Opens the inspector on `tab`, or closes it when that tab is already showing.
    pub(super) fn toggle_inspector_tab(&mut self, tab: InspectorTab) {
        if self.settings.show_sidebar && self.inspector_tab == tab {
            self.settings.show_sidebar = false;
        } else {
            self.settings.show_sidebar = true;
            self.inspector_tab = tab;
        }
    }

    fn apply_popover_action(&mut self, action: PopoverAction) {
        match action {
            PopoverAction::LoadHdri => self.open_hdri_dialog(),
            PopoverAction::SetDefault(env) => self.settings.default_environment = env,
            PopoverAction::RemoveEnvironment(path) => {
                self.settings.custom_environments.retain(|p| *p != path);
                let gone = Environment::File(path);
                if self.settings.default_environment == gone {
                    self.settings.default_environment = Environment::Preset(environment::Preset::Forest);
                }
                if self.settings.environment == gone {
                    self.settings.environment = self.settings.default_environment.clone();
                }
            }
        }
    }

    /// Channels worth offering for this model: the maps its materials use, plus the UV grid.
    pub(super) fn available_channels(&self) -> Vec<TexturePass> {
        let Some(info) = &self.info else { return Vec::new() };
        let present: Vec<TexturePass> = info
            .materials
            .iter()
            .enumerate()
            .filter(|(mi, _)| info.objects.iter().any(|o| o.material == *mi))
            .flat_map(|(_, m)| m.maps.iter().filter_map(|map| crate::ui::sidebar::pass_for(map.label)))
            .collect();
        TexturePass::ALL
            .into_iter()
            .filter(|p| *p == TexturePass::UvChecker || present.contains(p))
            .collect()
    }

    /// The channel the strip highlights: the active object's override when something is
    /// selected, otherwise the scene-wide texture pass.
    fn current_channel(&self) -> Option<TexturePass> {
        if self.selection.count() > 0 {
            self.selection.active.and_then(|a| self.pass_override[a])
        } else {
            (self.settings.shading == ShadingMode::Solid && self.settings.color == ColorMode::Texture)
                .then_some(self.settings.texture_pass)
        }
    }

    /// C / Shift+C: next / previous channel, passing through "off" (material colors).
    pub(super) fn cycle_channel(&mut self, forward: bool) {
        let channels = self.available_channels();
        if channels.is_empty() {
            return;
        }
        let n = channels.len() as isize + 1; // + off
        let at = self.current_channel().and_then(|c| channels.iter().position(|p| *p == c)).map_or(n - 1, |i| i as isize);
        let next = (at + if forward { 1 } else { -1 }).rem_euclid(n) as usize;
        self.apply_channel(channels.get(next).map_or(ChannelAction::Off, |p| ChannelAction::Show(*p)));
    }

    /// Texture channels as a vertical strip under the gizmo: one click, no menu over the model.
    pub(super) fn channel_strip(&mut self, ctx: &egui::Context, viewport: Rect, below_gizmo: bool) {
        let channels = self.available_channels();
        if channels.is_empty() {
            return;
        }
        let current = self.current_channel();
        let target = match self.selection.count() {
            0 => tr("All objects").to_string(),
            1 => tr("1 selected object").to_string(),
            n => trf("{n} selected objects", &[("n", &n)]),
        };
        let overrides = self.pass_override.iter().filter(|p| p.is_some()).count();
        let column = self.overlay_top(viewport, viewport.right() - 16.0 - gizmo::WIDTH, viewport.right() - 16.0);
        let top = column + if below_gizmo { gizmo::WIDTH + 12.0 } else { 0.0 };
        let mut action = None;
        egui::Area::new(Id::new("channel_strip"))
            .order(Order::Middle)
            .fixed_pos(pos2(viewport.right() - 16.0, top))
            .pivot(Align2::RIGHT_TOP)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(theme::RADIUS))
                    .inner_margin(Margin::same(4))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let width = 92.0;
                        let (head, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
                        let caption = ui.painter().layout_job(theme::caps(tr("Channel"), 10.0, theme::TEXT_FAINT));
                        ui.painter().galley(pos2(head.left() + 6.0, head.center().y - caption.size().y * 0.5), caption, theme::TEXT_FAINT);
                        for pass in &channels {
                            let selected = current == Some(*pass);
                            let (rect, r) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click());
                            let fill = if selected {
                                theme::ACCENT
                            } else if r.hovered() {
                                theme::WIDGET_HOVER
                            } else {
                                Color32::TRANSPARENT
                            };
                            let fg = if selected { theme::ON_ACCENT } else if r.hovered() { theme::TEXT } else { theme::TEXT_DIM };
                            ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS - 1), fill);
                            ui.painter().text(rect.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, tr(pass.label()), FontId::proportional(12.5), fg);
                            let tip = if selected { tr("Click again for material colors").to_string() } else { trf("Applies to: {target}", &[("target", &target)]) };
                            if r.on_hover_text(format!("{tip}\n{}", tr("C / Shift C: next / previous channel"))).clicked() {
                                action = Some(if selected { ChannelAction::Off } else { ChannelAction::Show(*pass) });
                            }
                        }
                        if overrides > 0 {
                            ui.add_space(2.0);
                            let (rect, r) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click());
                            let fg = if r.hovered() { theme::TEXT } else { theme::TEXT_DIM };
                            ui.painter().text(rect.center(), Align2::CENTER_CENTER, trf("Reset ({n})", &[("n", &overrides)]), FontId::proportional(11.5), fg);
                            if r.on_hover_text(tr("Remove channel overrides from every object")).clicked() {
                                action = Some(ChannelAction::ClearOverrides);
                            }
                        }
                    });
            });
        if let Some(a) = action {
            self.apply_channel(a);
        }
    }

    /// Measurements: the distance line, its X/Y/Z legs along the axes and a label with the
    /// length and the per-axis deltas. Drawn on top of the model so they're never hidden.
    pub(super) fn draw_measures(&self, ui: &Ui, viewport: Rect) {
        let preview = self.measure_start.zip(self.measure_hover.map(|h| h.0));
        let snapped = self.measure_hover.filter(|h| h.1 && self.tool == Tool::Measure).map(|h| h.0);
        if self.measures.is_empty() && self.measure_start.is_none() && snapped.is_none() {
            return;
        }
        let aspect = viewport.width() / viewport.height().max(1.0);
        let view_proj = self.camera.projection(aspect) * self.camera.view_matrix();
        let project = |p: Vec3| {
            let c = view_proj * p.extend(1.0);
            (c.w > 1e-6).then(|| {
                let n = c.truncate() / c.w;
                pos2(viewport.left() + (n.x * 0.5 + 0.5) * viewport.width(), viewport.top() + (0.5 - n.y * 0.5) * viewport.height())
            })
        };
        let painter = ui.painter().with_clip_rect(viewport);
        let shadow = Color32::from_black_alpha(170);
        let dot = |at: Pos2| {
            painter.circle(at, 4.0, theme::TEXT, Stroke::new(1.5, shadow));
        };
        let segments = self.measures.iter().map(|m| (m[0], m[1], false)).chain(preview.map(|(a, b)| (a, b, true)));
        for (a, b, live) in segments {
            let (Some(sa), Some(sb)) = (project(a), project(b)) else { continue };
            // Legs along X, then Y, then Z, like walking the bounding box of the segment.
            let corners = [a, Vec3::new(b.x, a.y, a.z), Vec3::new(b.x, b.y, a.z), b];
            let up = self.settings.up_axis;
            for k in 0..3 {
                let color = crate::axes::color(up.display(k).0);
                if (corners[k + 1] - corners[k]).length() < 1e-6 {
                    continue;
                }
                if let (Some(p), Some(q)) = (project(corners[k]), project(corners[k + 1])) {
                    painter.extend(egui::Shape::dashed_line(&[p, q], Stroke::new(1.2, color.gamma_multiply(0.85)), 5.0, 4.0));
                }
            }
            painter.line_segment([sa, sb], Stroke::new(4.0, shadow));
            painter.line_segment([sa, sb], Stroke::new(2.0, if live { theme::TEXT.gamma_multiply(0.8) } else { theme::TEXT }));
            dot(sa);
            dot(sb);

            let d = up.to_display(b - a);
            let mut job = LayoutJob::default();
            job.append(&fmt_len(d.length()), 0.0, TextFormat { font_id: theme::medium(14.0), color: theme::TEXT, ..Default::default() });
            job.append("\n", 0.0, TextFormat::default());
            for (k, axis) in crate::axes::NAMES.into_iter().enumerate() {
                let color = crate::axes::color(k);
                let sep = if k > 0 { "  " } else { "" };
                job.append(&format!("{sep}{axis} "), 0.0, TextFormat { font_id: theme::mono(10.5), color, ..Default::default() });
                job.append(&fmt_len(d[k].abs()), 0.0, TextFormat { font_id: theme::mono(10.5), color: theme::TEXT_DIM, ..Default::default() });
            }
            let galley = painter.layout_job(job);
            let mid = pos2((sa.x + sb.x) * 0.5, (sa.y + sb.y) * 0.5);
            let chip = Rect::from_center_size(mid - vec2(0.0, galley.size().y * 0.5 + 16.0), galley.size() + vec2(20.0, 12.0));
            painter.rect_filled(chip, CornerRadius::same(theme::RADIUS), Color32::from_black_alpha(215));
            painter.rect_stroke(chip, CornerRadius::same(theme::RADIUS), Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
            painter.galley(chip.min + vec2(10.0, 6.0), galley, theme::TEXT);
        }
        if let (Some(a), None) = (self.measure_start, preview) {
            if let Some(sa) = project(a) {
                dot(sa);
            }
        }
        // Snapped vertex under the cursor: a square, like Blender's vertex snap target.
        if let Some(at) = snapped.and_then(project) {
            let square = Rect::from_center_size(at, Vec2::splat(12.0));
            painter.rect_stroke(square, CornerRadius::ZERO, Stroke::new(4.0, shadow), egui::StrokeKind::Middle);
            painter.rect_stroke(square, CornerRadius::ZERO, Stroke::new(2.0, theme::TEXT), egui::StrokeKind::Middle);
        }
    }

    /// Object origins as dots (selected ones in the selection colors), like Blender, plus the
    /// active object's local axes so the pivot's orientation shows too.
    pub(super) fn draw_origins(&self, ui: &Ui, viewport: Rect) {
        let Some(info) = &self.info else { return };
        if !(self.settings.show_origins && self.settings.show_overlays) {
            return;
        }
        let aspect = viewport.width() / viewport.height().max(1.0);
        let view_proj = self.camera.projection(aspect) * self.camera.view_matrix();
        let painter = ui.painter().with_clip_rect(viewport);
        let project = |p: Vec3| {
            let c = view_proj * p.extend(1.0);
            (c.w > 1e-6).then(|| {
                let n = c.truncate() / c.w;
                pos2(viewport.left() + (n.x * 0.5 + 0.5) * viewport.width(), viewport.top() + (0.5 - n.y * 0.5) * viewport.height())
            })
        };
        if let Some(o) = self.selection.active.and_then(|i| info.objects.get(i)).filter(|_| self.selection.active.is_some_and(|i| self.visible.get(i).copied().unwrap_or(true))) {
            // A fixed share of the camera distance keeps the axes about the same size on screen.
            let length = (self.camera.eye() - o.origin).length() * 0.08;
            if let Some(at) = project(o.origin) {
                for (axis, color) in o.axes.iter().zip([theme::AXIS_X, theme::AXIS_Y, theme::AXIS_Z]) {
                    if let Some(tip) = project(o.origin + *axis * length) {
                        painter.line_segment([at, tip], Stroke::new(4.0, Color32::from_black_alpha(160)));
                        painter.line_segment([at, tip], Stroke::new(2.0, color));
                    }
                }
            }
        }
        for (i, o) in info.objects.iter().enumerate() {
            if !self.visible.get(i).copied().unwrap_or(true) {
                continue;
            }
            let Some(at) = project(o.origin) else { continue };
            let fill = if self.selection.active == Some(i) {
                Color32::from_rgb(0xff, 0xab, 0x40)
            } else if self.selection.selected.get(i).copied().unwrap_or(false) {
                theme::SELECTION
            } else {
                theme::TEXT
            };
            painter.circle(at, 3.5, fill, Stroke::new(1.5, Color32::from_black_alpha(200)));
        }
    }

    /// Darkens areas outside the target render aspect ratio (passepartout) when the Render tab is open.
    pub(super) fn draw_render_framing_guide(&self, ui: &Ui, viewport: Rect) {
        if !self.settings.show_sidebar
            || self.inspector_tab != InspectorTab::Render
            || !self.settings.render_framing_guide
        {
            return;
        }
        let [target_w, target_h] = self.export_resolution_px();
        if target_w == 0 || target_h == 0 {
            return;
        }
        let target_aspect = target_w as f32 / target_h as f32;
        let vp_aspect = viewport.width() / viewport.height().max(1.0);

        if (target_aspect - vp_aspect).abs() < 0.005 {
            return;
        }

        let painter = ui.painter().with_clip_rect(viewport);
        let mask_color = Color32::from_black_alpha(150);
        let border_stroke = Stroke::new(1.5, Color32::from_white_alpha(160));

        if target_aspect < vp_aspect {
            let frame_w = viewport.height() * target_aspect;
            let x0 = viewport.center().x - frame_w * 0.5;
            let x1 = viewport.center().x + frame_w * 0.5;

            let left_rect = Rect::from_min_max(viewport.min, pos2(x0, viewport.max.y));
            let right_rect = Rect::from_min_max(pos2(x1, viewport.min.y), viewport.max);
            let frame_rect = Rect::from_min_max(pos2(x0, viewport.min.y), pos2(x1, viewport.max.y));

            painter.rect_filled(left_rect, CornerRadius::ZERO, mask_color);
            painter.rect_filled(right_rect, CornerRadius::ZERO, mask_color);
            painter.rect_stroke(frame_rect, CornerRadius::ZERO, border_stroke, StrokeKind::Inside);
        } else {
            let frame_h = viewport.width() / target_aspect;
            let y0 = viewport.center().y - frame_h * 0.5;
            let y1 = viewport.center().y + frame_h * 0.5;

            if y0 >= viewport.min.y && y1 <= viewport.max.y {
                let top_rect = Rect::from_min_max(viewport.min, pos2(viewport.max.x, y0));
                let bottom_rect = Rect::from_min_max(pos2(viewport.min.x, y1), viewport.max);
                let frame_rect = Rect::from_min_max(pos2(viewport.min.x, y0), pos2(viewport.max.x, y1));

                painter.rect_filled(top_rect, CornerRadius::ZERO, mask_color);
                painter.rect_filled(bottom_rect, CornerRadius::ZERO, mask_color);
                painter.rect_stroke(frame_rect, CornerRadius::ZERO, border_stroke, StrokeKind::Inside);
            }
        }
    }

    /// Bounding box cage and X, Y, Z extent dimensions (dashed lines with colored axis labels).
    pub(super) fn draw_bounds_overlay(&self, ui: &Ui, viewport: Rect) {
        if !self.settings.show_overlays || !self.settings.show_bounds_overlay {
            return;
        }
        if self.info.is_none() {
            return;
        }
        let b = self.visible_bounds(false);
        if !b.is_valid() {
            return;
        }
        let aspect = viewport.width() / viewport.height().max(1.0);
        let view_proj = self.camera.projection(aspect) * self.camera.view_matrix();
        let project = |p: Vec3| {
            let c = view_proj * p.extend(1.0);
            (c.w > 1e-6).then(|| {
                let n = c.truncate() / c.w;
                pos2(viewport.left() + (n.x * 0.5 + 0.5) * viewport.width(), viewport.top() + (0.5 - n.y * 0.5) * viewport.height())
            })
        };
        let painter = ui.painter().with_clip_rect(viewport);
        let (min, max) = (b.min, b.max);
        let size = b.size();

        // 8 corners
        let c000 = min;
        let c100 = Vec3::new(max.x, min.y, min.z);
        let c010 = Vec3::new(min.x, max.y, min.z);
        let c110 = Vec3::new(max.x, max.y, min.z);
        let c001 = Vec3::new(min.x, min.y, max.z);
        let c101 = Vec3::new(max.x, min.y, max.z);
        let c011 = Vec3::new(min.x, max.y, max.z);
        let c111 = max;

        // Faint cage for the 12 edges
        let edges = [
            (c000, c100), (c010, c110), (c001, c101), (c011, c111),
            (c000, c010), (c100, c110), (c001, c011), (c101, c111),
            (c000, c001), (c100, c101), (c010, c011), (c110, c111),
        ];

        let cage_stroke = Stroke::new(1.0, Color32::from_black_alpha(100));
        for (p0, p1) in edges {
            if let (Some(s0), Some(s1)) = (project(p0), project(p1)) {
                painter.extend(egui::Shape::dashed_line(&[s0, s1], cage_stroke, 4.0, 4.0));
            }
        }

        // Main 3 dimension axes from the lowest corner with distinct axis colors
        let up = self.settings.up_axis;
        let axis_segments = [
            (0, c000, c100, size.x), // X
            (1, c000, c010, size.y), // Y
            (2, c000, c001, size.z), // Z
        ];

        for (axis_idx, p0, p1, len) in axis_segments {
            if len < 1e-5 {
                continue;
            }
            let (Some(s0), Some(s1)) = (project(p0), project(p1)) else { continue };
            let color = crate::axes::color(up.display(axis_idx).0);
            let stroke = Stroke::new(1.8, color);
            painter.extend(egui::Shape::dashed_line(&[s0, s1], stroke, 6.0, 4.0));

            // Dimension label badge along the edge
            let mid = pos2((s0.x + s1.x) * 0.5, (s0.y + s1.y) * 0.5);
            let axis_name = crate::axes::NAMES[up.display(axis_idx).0];
            let label = format!("{axis_name} {}", fmt_len(len));
            let galley = painter.layout_job(theme::caps(&label, 11.0, theme::TEXT));
            let chip = Rect::from_center_size(mid, galley.size() + vec2(14.0, 8.0));
            painter.rect_filled(chip, CornerRadius::same(theme::RADIUS), Color32::from_black_alpha(215));
            painter.rect_stroke(chip, CornerRadius::same(theme::RADIUS), Stroke::new(1.0, color.gamma_multiply(0.8)), StrokeKind::Inside);
            painter.galley(chip.min + vec2(7.0, 4.0), galley, theme::TEXT);
        }
    }

    /// Viewport light locators / gizmos in photo mode style: visible position, color, draggable in 3D view.
    pub(super) fn draw_lights_overlay(&mut self, ui: &mut Ui, viewport: Rect) {
        // The overlays switch hides them too, even while the Render tab is open.
        if self.settings.shading != ShadingMode::Rendered || !self.settings.show_overlays {
            return;
        }
        let in_render_tab = self.settings.show_sidebar && self.inspector_tab == InspectorTab::Render;
        if !self.settings.show_light_gizmos && !in_render_tab {
            return;
        }
        if self.info.is_none() {
            return;
        }
        let b = self.visible_bounds(false);
        let center = if b.is_valid() { b.center() } else { Vec3::ZERO };
        let radius = if b.is_valid() { b.radius().max(0.5) } else { 2.0 };
        let aspect = viewport.width() / viewport.height().max(1.0);
        let view_proj = self.camera.projection(aspect) * self.camera.view_matrix();
        let project = |p: Vec3| {
            let c = view_proj * p.extend(1.0);
            (c.w > 1e-6).then(|| {
                let n = c.truncate() / c.w;
                pos2(viewport.left() + (n.x * 0.5 + 0.5) * viewport.width(), viewport.top() + (0.5 - n.y * 0.5) * viewport.height())
            })
        };
        let painter = ui.painter().with_clip_rect(viewport);
        let center_pt = project(center);

        let orbit = radius * 1.5;
        let mut drag = None;

        for (i, light) in self.settings.lights.iter().enumerate() {
            if !light.enabled {
                continue;
            }
            let dir = if i == 0 && light.follow_env {
                let d = self.renderer.as_ref().map(|r| r.key_light()).unwrap_or(Vec3::new(0.5, 0.5, 0.7));
                let (sin, cos) = self.settings.env_rotation.to_radians().sin_cos();
                let d = Vec3::new(cos * d.x - sin * d.y, sin * d.x + cos * d.y, d.z);
                let flat = Vec3::new(d.x, d.y, 0.0).normalize_or(Vec3::X);
                let elevation = d.z.clamp(-1.0, 1.0).asin().max(25f32.to_radians());
                (flat * elevation.cos() + Vec3::Z * elevation.sin()).normalize()
            } else {
                // Same range the renderer lights with: the key light never goes below 5°.
                let (lo, hi) = light_pitch_range(i);
                let yaw_rad = light.yaw.to_radians();
                let pitch_rad = light.pitch.clamp(lo, hi).to_radians();
                let (sy, cy) = yaw_rad.sin_cos();
                let (sp, cp) = pitch_rad.sin_cos();
                Vec3::new(cp * cy, cp * sy, sp).normalize()
            };
            let world_pos = center + dir * orbit;
            let Some(screen_pos) = project(world_pos) else { continue };

            // Ray pointing towards center
            if let Some(cp) = center_pt {
                let ray_stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(light.color[0], light.color[1], light.color[2], 90));
                painter.extend(egui::Shape::dashed_line(&[screen_pos, cp], ray_stroke, 4.0, 4.0));
            }

            // Interactive light handle
            let handle_id = Id::new(("light_gizmo", i));
            let handle_rect = Rect::from_center_size(screen_pos, Vec2::splat(26.0));
            let r = ui.interact(handle_rect, handle_id, Sense::drag());
            if r.hovered() || r.dragged() {
                ui.ctx().set_cursor_icon(if r.dragged() { CursorIcon::Grabbing } else { CursorIcon::Grab });
            }
            if r.dragged() && r.drag_delta() != Vec2::ZERO {
                ui.ctx().request_repaint();
                drag = Some((i, screen_pos + r.drag_delta(), world_pos));
            }

            let light_color = Color32::from_rgb(light.color[0], light.color[1], light.color[2]);
            // Glow ring
            let ring_r = if r.hovered() || r.dragged() { 13.0 } else { 10.0 };
            painter.circle_filled(screen_pos, ring_r + 2.0, Color32::from_black_alpha(180));
            painter.circle_stroke(screen_pos, ring_r, Stroke::new(2.0, light_color));
            painter.circle_filled(screen_pos, ring_r * 0.55, light_color);

            // Light label chip
            let label = if light.name.is_empty() { format!("L{}", i + 1) } else { light.name.clone() };
            let galley = painter.layout_job(theme::caps(&label, 10.0, theme::TEXT));
            let chip = Rect::from_min_size(screen_pos + vec2(14.0, -8.0), galley.size() + vec2(8.0, 4.0));
            painter.rect_filled(chip, CornerRadius::same(2), Color32::from_black_alpha(200));
            painter.galley(chip.min + vec2(4.0, 2.0), galley, theme::TEXT);
        }

        // The handle follows the cursor on the sphere the lights sit on: cast the pointer's ray,
        // take the hit nearest to where the light was (so it stays on the side it's on), or the
        // sphere's rim when the ray misses it. Screen motion maps to the same visual motion from
        // any camera angle.
        let Some((idx, target, previous)) = drag else { return };
        let inv = view_proj.inverse();
        let ndc = vec2(
            (target.x - viewport.left()) / viewport.width() * 2.0 - 1.0,
            1.0 - (target.y - viewport.top()) / viewport.height() * 2.0,
        );
        // Reverse-Z: depth 1 is the near plane.
        let near = inv.project_point3(Vec3::new(ndc.x, ndc.y, 1.0));
        let far = inv.project_point3(Vec3::new(ndc.x, ndc.y, 0.5));
        let ray = (far - near).normalize_or_zero();
        if ray == Vec3::ZERO {
            return;
        }
        let oc = near - center;
        let b = oc.dot(ray);
        let disc = b * b - (oc.length_squared() - orbit * orbit);
        let hit = if disc >= 0.0 {
            let root = disc.sqrt();
            let (p1, p2) = (near + ray * (-b - root), near + ray * (-b + root));
            if p1.distance_squared(previous) <= p2.distance_squared(previous) { p1 } else { p2 }
        } else {
            near + ray * -b
        };
        let d = (hit - center).normalize_or_zero();
        if d == Vec3::ZERO {
            return;
        }
        if let Some(light) = self.settings.lights.get_mut(idx) {
            let (lo, hi) = light_pitch_range(idx);
            light.follow_env = false;
            light.yaw = d.y.atan2(d.x).to_degrees().rem_euclid(360.0);
            light.pitch = d.z.clamp(-1.0, 1.0).asin().to_degrees().clamp(lo, hi);
        }
    }

    /// Outline of the section plane across the scene bounds.
    pub(super) fn draw_section(&self, ui: &Ui, viewport: Rect) {
        let (Some(s), Some(plane)) = (self.section, self.section_plane()) else { return };
        let b = self.scene_bounds();
        let at = plane[3] * plane[s.axis];
        let (u, v) = ((s.axis + 1) % 3, (s.axis + 2) % 3);
        let corner = |cu: f32, cv: f32| {
            let mut p = Vec3::ZERO;
            p[s.axis] = at;
            p[u] = cu;
            p[v] = cv;
            p
        };
        let corners = [corner(b.min[u], b.min[v]), corner(b.max[u], b.min[v]), corner(b.max[u], b.max[v]), corner(b.min[u], b.max[v])];
        let aspect = viewport.width() / viewport.height().max(1.0);
        let view_proj = self.camera.projection(aspect) * self.camera.view_matrix();
        let project = |p: Vec3| {
            let c = view_proj * p.extend(1.0);
            (c.w > 1e-6).then(|| {
                let n = c.truncate() / c.w;
                pos2(viewport.left() + (n.x * 0.5 + 0.5) * viewport.width(), viewport.top() + (0.5 - n.y * 0.5) * viewport.height())
            })
        };
        let Some(points) = corners.iter().map(|&c| project(c)).collect::<Option<Vec<Pos2>>>() else { return };
        let painter = ui.painter().with_clip_rect(viewport);
        let color = theme::SECTION;
        let mut closed = points.clone();
        closed.push(points[0]);
        painter.extend(egui::Shape::dashed_line(&closed, Stroke::new(1.5, color), 8.0, 5.0));
    }

    /// Axis, position, flip and close for the cross-section, docked at the bottom of the view.
    pub(super) fn section_bar(&mut self, ctx: &egui::Context, viewport: Rect) {
        let Some(mut s) = self.section else { return };
        let b = self.scene_bounds();
        let mut close = false;
        egui::Area::new(Id::new("section_bar"))
            .order(Order::Middle)
            .fixed_pos(pos2(viewport.center().x, viewport.bottom() - 12.0))
            .pivot(Align2::CENTER_BOTTOM)
            .show(ctx, |ui| {
                toolbar(ui, |ui| {
                    ui.add_space(6.0);
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                    icons::section(ui.painter(), r, theme::SECTION);
                    ui.label(theme::caps(tr("Section"), 11.0, theme::TEXT_DIM));
                    toolbar_separator(ui);
                    // Buttons name display axes; the plane itself stays on the world axis.
                    let up = self.settings.up_axis;
                    for (d, label) in crate::axes::NAMES.into_iter().enumerate() {
                        let axis = up.internal(d).0;
                        if text_button(ui, label, s.axis == axis).on_hover_text(tr("Cut across this axis")).clicked() {
                            s.axis = axis;
                        }
                    }
                    toolbar_separator(ui);
                    ui.spacing_mut().slider_width = 180.0;
                    // A display axis pointing the other way (Y up shows world -Y as Z) keeps the
                    // slider growing with the displayed coordinate.
                    let sign = up.display(s.axis).1;
                    let mut shown_t = if sign < 0.0 { 1.0 - s.t } else { s.t };
                    if ui
                        .add(egui::Slider::new(&mut shown_t, 0.0..=1.0).show_value(false))
                        .on_hover_text(tr("Drag in the view with the Section tool to move it"))
                        .changed()
                    {
                        s.t = if sign < 0.0 { 1.0 - shown_t } else { shown_t };
                    }
                    let extent = b.max[s.axis] - b.min[s.axis];
                    let at = sign * (b.min[s.axis] + extent * s.t);
                    // Float noise around the origin reads as "0,01 µm": show a clean zero.
                    let at = if at.abs() < extent * 1e-4 { 0.0 } else { at };
                    let (r, _) = ui.allocate_exact_size(vec2(64.0, theme::TOOLBAR_HEIGHT), Sense::hover());
                    ui.painter().text(r.left_center(), Align2::LEFT_CENTER, fmt_len(at), theme::mono(12.0), theme::TEXT);
                    toolbar_separator(ui);
                    if text_button(ui, "Flip", s.flip).on_hover_text(tr("Keep the other side")).clicked() {
                        s.flip = !s.flip;
                    }
                    close = widgets::sized_icon_button(ui, icons::close, false, Vec2::splat(theme::TOOLBAR_HEIGHT))
                        .on_hover_text(tr("Turn the section off"))
                        .clicked();
                });
            });
        if close {
            self.section = None;
            if self.tool == Tool::Section {
                self.tool = Tool::Select;
            }
        } else {
            self.section = Some(s);
        }
    }

    /// Top of an overlay spanning `left..right` along the viewport's top edge: 16 px down, or
    /// below the viewport toolbar when the two would overlap (small window, large UI scale,
    /// inspector open). The toolbar keeps its place; the HUD chips, gizmo and channel strip move.
    pub(super) fn overlay_top(&self, viewport: Rect, left: f32, right: f32) -> f32 {
        let bar = self.toolbar_rect;
        if bar.is_positive() && left < bar.right() + 8.0 && right > bar.left() - 8.0 {
            bar.bottom() + 8.0
        } else {
            viewport.top() + 16.0
        }
    }

    /// Top-left chip ("USER PERSPECTIVE · SOLID"), optional stats, and the grid scale bar.
    pub(super) fn hud(&self, ui: &Ui, viewport: Rect) {
        let painter = ui.painter();
        let shading = match self.settings.shading {
            ShadingMode::Wireframe => "Wireframe",
            ShadingMode::Solid => "Solid",
            ShadingMode::Rendered => "Rendered",
        };
        let mut lines = vec![format!("{} · {}", self.camera.view_name(), tr(shading))];
        if self.settings.show_stats {
            if let Some(info) = &self.info {
                let selected = self.selection.count();
                let objects = if selected > 0 { format!("{selected}/{}", info.objects.len()) } else { info.objects.len().to_string() };
                lines.push(trf(
                    "{objects} objects · {tris} tris · {verts} verts",
                    &[("objects", &objects), ("tris", &thousands(info.triangles)), ("verts", &thousands(info.vertices))],
                ));
            }
        }
        let galleys: Vec<_> = lines.iter().map(|l| painter.layout_job(theme::caps(l, 11.0, theme::TEXT_DIM))).collect();
        let widest = galleys.iter().map(|g| g.size().x + 20.0).fold(0.0, f32::max);
        let mut y = self.overlay_top(viewport, viewport.left() + 16.0, viewport.left() + 16.0 + widest);
        for galley in galleys {
            let chip = Rect::from_min_size(pos2(viewport.left() + 16.0, y), galley.size() + vec2(20.0, 12.0));
            painter.rect_filled(chip, CornerRadius::same(2), Color32::from_black_alpha(204));
            painter.galley(chip.min + vec2(10.0, 6.0), galley, theme::TEXT_DIM);
            y = chip.bottom() + 6.0;
        }

        if self.settings.show_grid && self.info.is_some() {
            let level = self.camera.view.distance.max(1e-6).log10() - GRID_LEVEL_OFFSET;
            let fine = 10f32.powf(level.floor());
            let cell = if level - level.floor() > 0.5 { fine * 10.0 } else { fine };
            let base = pos2(viewport.left() + 16.0, viewport.bottom() - 22.0);
            let stroke = Stroke::new(1.0, theme::TEXT_DIM);
            painter.line_segment([base, base + vec2(48.0, 0.0)], stroke);
            painter.line_segment([base, base - vec2(0.0, 6.0)], stroke);
            painter.line_segment([base + vec2(48.0, 0.0), base + vec2(48.0, -6.0)], stroke);
            let label = painter.layout_job(theme::caps(&fmt_len(cell), 11.0, theme::TEXT_DIM));
            painter.galley(base + vec2(56.0, -label.size().y * 0.5 - 2.0), label, theme::TEXT_DIM);
        }
    }

    // --- Inspector -------------------------------------------------------------------------------

    pub(super) fn inspector(&mut self, ui: &mut Ui) {
        // Tabs.
        ui.add_space(8.0);
        let mut tabs = vec![
            (InspectorTab::Info, "Info"),
            (InspectorTab::Shading, "Shading"),
            (InspectorTab::Render, "Render"),
        ];
        if !self.manufacturing() {
            tabs.push((InspectorTab::Materials, "Materials"));
        }
        tabs.push((InspectorTab::Scene, "Scene"));

        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let total_w = ui.available_width() - 8.0;
            Frame::new()
                .fill(theme::SURFACE)
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(theme::RADIUS))
                .inner_margin(Margin::symmetric(2, 2))
                .show(ui, |ui| {
                    ui.set_width(total_w);
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let count = tabs.len() as f32;
                    let item_w = ((total_w - 4.0 - 2.0 * (count - 1.0)) / count).floor();
                    ui.horizontal(|ui| {
                        for (tab, label) in tabs {
                            let selected = self.inspector_tab == tab;
                            let (rect, r) = ui.allocate_exact_size(vec2(item_w, 28.0), Sense::click());
                            let fill = if selected {
                                theme::ACCENT
                            } else if r.hovered() {
                                theme::WIDGET_HOVER
                            } else {
                                Color32::TRANSPARENT
                            };
                            ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS - 1), fill);
                            let color = if selected {
                                theme::ON_ACCENT
                            } else if r.hovered() {
                                theme::TEXT
                            } else {
                                theme::TEXT_DIM
                            };
                            let font = if selected { theme::bold(12.0) } else { theme::medium(12.0) };
                            ui.painter().text(rect.center(), Align2::CENTER_CENTER, tr(label), font, color);
                            if r.clicked() {
                                self.inspector_tab = tab;
                            }
                        }
                    });
                });
        });
        ui.add_space(6.0);
        let line = ui.max_rect().x_range();
        let y = ui.cursor().top();
        ui.painter().hline(line, y, Stroke::new(1.0, theme::BORDER));

        // Viewport and render settings don't need a model.
        if self.inspector_tab == InspectorTab::Shading {
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                    widgets::begin_cards(ui);
                    self.shading_tab(ui);
                    widgets::end_cards(ui);
                });
            });
            return;
        }
        if self.inspector_tab == InspectorTab::Render {
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                    widgets::begin_cards(ui);
                    self.render_tab(ui);
                    widgets::end_cards(ui);
                });
            });
            return;
        }
        if self.info.is_none() {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(RichText::new(tr("Open a file to see its objects.")).color(theme::TEXT_DIM));
            });
            return;
        }
        match self.inspector_tab {
            InspectorTab::Info => {
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                        widgets::begin_cards(ui);
                        self.info_tab(ui);
                        widgets::end_cards(ui);
                    });
                });
            }
            InspectorTab::Materials => {
                ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
                        widgets::begin_cards(ui);
                        self.materials_tab(ui);
                        widgets::end_cards(ui);
                    });
                });
            }
            InspectorTab::Scene => Frame::new().inner_margin(Margin::same(8)).show(ui, |ui| self.scene_tab(ui)).inner,
            InspectorTab::Shading | InspectorTab::Render => {}
        }
    }

    /// Settings of the current shading mode. The mode switch sits on top, so it's clear the
    /// options below belong to that mode and change with it.
    fn shading_tab(&mut self, ui: &mut Ui) {
        widgets::section(ui, "Mode");
        ui.horizontal(|ui| {
            widgets::segmented(
                ui,
                &mut self.settings.shading,
                &[(ShadingMode::Wireframe, "Wireframe"), (ShadingMode::Solid, "Solid"), (ShadingMode::Rendered, "Rendered")],
            )
        });
        ui.label(RichText::new(tr("The options below change with the mode.")).size(11.0).color(theme::TEXT_DIM));
        let manufacturing = self.manufacturing();
        if manufacturing && self.settings.shading != ShadingMode::Wireframe {
            popovers::part_material(ui, &mut self.settings);
        }
        let action = popovers::shading(ui, &mut self.settings, &mut self.thumbs, manufacturing);
        if let Some(a) = action {
            self.apply_popover_action(a);
        }
    }

    /// Dedicated Render tab: resolution, supersampling, lights and shadows, turntable and export.
    fn render_tab(&mut self, ui: &mut Ui) {
        let has_model = self.info.is_some();
        let ctx = ui.ctx().clone();
        let mut do_export_image = false;
        let mut do_export_turntable = false;
        let mut do_split_shading = false;
        let busy = self.turntable_job.is_some();
        let view_px = self.export_view_px();

        {
            let s = &mut self.settings;

            // --- SHADING QUICK SWITCH ---
            widgets::section(ui, "Shading Mode");
            ui.horizontal(|ui| {
                widgets::segmented(
                    ui,
                    &mut s.shading,
                    &[(ShadingMode::Wireframe, "Wireframe"), (ShadingMode::Solid, "Solid"), (ShadingMode::Rendered, "Rendered")],
                );
            });

            // --- RESOLUTION & QUALITY ---
            widgets::section(ui, "Resolution");
            let current_label = s.render_resolution.label();
            egui::ComboBox::from_id_salt("render_res_preset")
                .selected_text(tr(current_label))
                .width(ui.available_width() - 8.0)
                .show_ui(ui, |ui| {
                    for r in RenderResolution::ALL {
                        ui.selectable_value(&mut s.render_resolution, r, tr(r.label()));
                    }
                });

            if s.render_resolution == RenderResolution::Custom {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(tr("Size:")).color(theme::TEXT_DIM));
                    ui.add(egui::DragValue::new(&mut s.render_custom_w).range(64..=8192).suffix(" px").speed(2.0));
                    ui.label("×");
                    ui.add(egui::DragValue::new(&mut s.render_custom_h).range(64..=8192).suffix(" px").speed(2.0));
                });
            }

            let [eff_w, eff_h] = s.render_resolution.dimensions(view_px, s.render_custom_w, s.render_custom_h);
            ui.label(
                RichText::new(trf("Output: {w} × {h} px", &[("w", &eff_w), ("h", &eff_h)]))
                    .size(11.0)
                    .color(theme::TEXT_DIM),
            );

            widgets::section(ui, "Quality (SSAA)");
            ui.horizontal(|ui| {
                widgets::segmented(
                    ui,
                    &mut s.render_ssaa,
                    &[(1, "1× Normal"), (2, "2× High"), (4, "4× Ultra")],
                );
            });
            let ssaa_hint = match s.render_ssaa {
                1 => tr("Standard 4× MSAA. Fastest rendering."),
                2 => tr("2× Supersampling (SSAA) + Lanczos3 filter. Razor-sharp speculars and edges."),
                _ => tr("4× Ultra Supersampling. Master resolution for prints and fine details."),
            };
            ui.label(RichText::new(ssaa_hint).size(11.0).color(theme::TEXT_DIM));

            ui.add_space(4.0);
            ui.checkbox(&mut s.render_framing_guide, tr("Framing guide (Passepartout)"))
                .on_hover_text(tr("Darkens regions outside the render aspect ratio in the viewport"));

            ui.checkbox(&mut s.export_transparent, tr("Transparent background"))
                .on_hover_text(tr("Export PNG with alpha transparency (disables background sky)"));

            ui.checkbox(&mut s.export_grid, tr("Include floor grid"));

            ui.add_space(6.0);
            ui.add_enabled_ui(has_model, |ui| {
                if primary_button(ui, "Render Image…", 36.0, Some("F12")).on_hover_text(tr("Render and save image (F12)")).clicked() {
                    do_export_image = true;
                }
            });
            if !has_model {
                ui.label(RichText::new(tr("Open a file to render.")).size(11.0).color(theme::TEXT_DIM));
            }

            ui.separator();

            // --- LIGHTS AND SHADOWS ---
            widgets::section(ui, "Lights & Shadows");

            ui.checkbox(&mut s.show_light_gizmos, tr("Show light gizmos in viewport"))
                .on_hover_text(tr("Display interactive light handles in the 3D viewport (Photo Mode)"));

            ui.checkbox(&mut s.shadows, tr("Model shadows"))
                .on_hover_text(tr("Primary light casts shadows on the model"));
            ui.checkbox(&mut s.floor_shadow, tr("Shadow on the floor"))
                .on_hover_text(tr("A floor under the model that catches its shadow and contact shading"));
            ui.add_enabled_ui(s.shadows || s.floor_shadow, |ui| {
                ui.add(egui::Slider::new(&mut s.shadow_softness, 0.0..=1.0).text(tr("Softness")));
            });

            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.label(RichText::new(tr("Studio Lights")).strong());
                ui.label(RichText::new(format!("({}/6)", s.lights.len())).size(11.0).color(theme::TEXT_DIM));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if s.lights.len() < 6 {
                        if ui.small_button(tr("+ Add Light")).on_hover_text(tr("Add a new custom light source (up to 6)")).clicked() {
                            let idx = s.lights.len() + 1;
                            let yaw = (s.lights.len() as f32 * 60.0 + 35.0).rem_euclid(360.0);
                            s.lights.push(crate::settings::CustomLight {
                                name: format!("Light {idx}"),
                                enabled: true,
                                follow_env: false,
                                yaw,
                                pitch: 45.0,
                                strength: 1.0,
                                color: [255, 255, 255],
                            });
                        }
                    }
                });
            });

            let mut remove_light_idx = None;
            let total_lights = s.lights.len();

            for (idx, light) in s.lights.iter_mut().enumerate() {
                ui.add_space(2.0);
                Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(theme::RADIUS))
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut light.enabled, "");
                            let name_label = if idx == 0 {
                                tr("Key Light")
                            } else if idx == 1 {
                                tr("Fill Light")
                            } else {
                                light.name.as_str()
                            };
                            let text_color = if light.enabled { theme::TEXT } else { theme::TEXT_DIM };
                            ui.label(RichText::new(name_label).strong().color(text_color));

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if total_lights > 1 {
                                    if ui.small_button("✕").on_hover_text(tr("Remove this light")).clicked() {
                                        remove_light_idx = Some(idx);
                                    }
                                }
                                ui.color_edit_button_srgb(&mut light.color);
                            });
                        });

                        if light.enabled {
                            ui.add_space(4.0);
                            if idx == 0 {
                                ui.horizontal(|ui| {
                                    widgets::segmented(
                                        ui,
                                        &mut light.follow_env,
                                        &[(true, "Follow HDRI"), (false, "Custom")],
                                    );
                                });
                                if light.follow_env {
                                    ui.label(
                                        RichText::new(tr("Tracks the brightest point in the sky; turns with Environment rotation."))
                                            .size(11.0)
                                            .color(theme::TEXT_DIM),
                                    );
                                }
                            }
                            if !light.follow_env {
                                ui.add(egui::Slider::new(&mut light.yaw, 0.0..=360.0).suffix("°").text(tr("Angle (Yaw)")));
                                ui.add(egui::Slider::new(&mut light.pitch, -60.0..=85.0).suffix("°").text(tr("Elevation (Pitch)")));
                            }
                            ui.add(egui::Slider::new(&mut light.strength, 0.0..=3.0).text(tr("Intensity")));

                            ui.horizontal(|ui| {
                                ui.label(RichText::new(tr("Presets:")).size(10.5).color(theme::TEXT_DIM));
                                for (col, tip) in [
                                    ([255, 255, 255], "Pure White"),
                                    ([255, 224, 178], "Warm 3200K"),
                                    ([255, 244, 229], "Daylight 5500K"),
                                    ([227, 242, 253], "Cool 6500K"),
                                    ([255, 213, 79], "Golden Hour"),
                                ] {
                                    let (rect, r) = ui.allocate_exact_size(Vec2::splat(13.0), Sense::click());
                                    ui.painter().rect_filled(rect, CornerRadius::same(2), Color32::from_rgb(col[0], col[1], col[2]));
                                    if r.on_hover_text(tr(tip)).clicked() {
                                        light.color = col;
                                    }
                                }
                            });
                        }
                    });
            }

            if let Some(remove_idx) = remove_light_idx {
                s.lights.remove(remove_idx);
            }

            // Sync legacy fields
            if let Some(l0) = s.lights.first() {
                s.light_follow_env = l0.follow_env;
                s.light_yaw = l0.yaw;
                s.light_pitch = l0.pitch;
                s.light_strength = l0.strength;
                s.light_color = l0.color;
            }
            if let Some(l1) = s.lights.get(1) {
                s.light2_enabled = l1.enabled;
                s.light2_yaw = l1.yaw;
                s.light2_pitch = l1.pitch;
                s.light2_strength = l1.strength;
                s.light2_color = l1.color;
            }

            ui.add_space(8.0);
            widgets::section(ui, "Split Shading (Compare)");
            ui.horizontal(|ui| {
                ui.add_enabled_ui(has_model, |ui| {
                    if ui.button(tr("Compare Shading Styles…")).on_hover_text(tr("Split view: compare two shading styles on this model")).clicked() {
                        do_split_shading = true;
                    }
                });
            });

            ui.separator();

            // --- TURNTABLE ---
            widgets::section(ui, "Turntable (360° Animation)");
            ui.horizontal(|ui| {
                widgets::segmented(ui, &mut s.turntable_mp4, &[(false, "GIF"), (true, "MP4")]);
            });
            if s.turntable_mp4 && !super::turntable::ffmpeg_available() {
                ui.label(
                    RichText::new(tr("MP4 needs ffmpeg on PATH (winget install ffmpeg). GIF works natively."))
                        .size(11.0)
                        .color(theme::ERROR),
                );
            }
            ui.horizontal(|ui| {
                widgets::segmented(ui, &mut s.turntable_size, &[(480, "480"), (720, "720"), (1080, "1080")]);
            });
            ui.add(egui::Slider::new(&mut s.turntable_seconds, 2.0..=12.0).step_by(0.5).suffix(" s").text(tr("Length")));
            ui.add_enabled_ui(self.anim.as_ref().is_some_and(|a| a.has_clips()), |ui| {
                ui.checkbox(&mut s.turntable_animate, tr("Play clip during turntable"));
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(has_model && !busy, |ui| {
                    if ui.button(tr("Export Turntable…")).clicked() {
                        do_export_turntable = true;
                    }
                });
                if busy {
                    ui.add(egui::Spinner::new().size(14.0));
                    ui.label(RichText::new(tr("Encoding…")).size(11.0).color(theme::TEXT_DIM));
                }
            });
        }

        if do_export_image {
            self.export_image(&ctx);
        }
        if do_export_turntable {
            self.export_turntable(&ctx);
        }
        if do_split_shading {
            self.open_compare_same_model(&ctx);
        }
    }

    fn info_tab(&mut self, ui: &mut Ui) {
        let Some(info) = &self.info else { return };
        ui.spacing_mut().item_spacing.y = 10.0;
        widgets::section(ui, "Geometry");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let w = ((ui.available_width() - 16.0) / 3.0).floor();
            stat_card(ui, &thousands(info.objects.len()), "Objects", w);
            stat_card(ui, &thousands(info.vertices), "Vertices", w);
            stat_card(ui, &thousands(info.triangles), "Triangles", w);
        });
        budget_bar(ui, &mut self.settings.triangle_budget, info.triangles);
        ui.add_space(12.0);
        self.compare_table(ui);

        widgets::section(ui, "Mesh Check");
        match &info.qa {
            None => {
                ui.label(RichText::new(tr("Analyzing…")).color(theme::TEXT_DIM));
            }
            Some(qa) => {
                let s = &mut self.settings;
                Frame::new()
                    .stroke(Stroke::new(1.0, theme::BORDER))
                    .corner_radius(CornerRadius::same(theme::RADIUS))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let rows = [
                            (qa.non_manifold_edges, "Non-manifold edges", theme::MARK_NON_MANIFOLD, Some(&mut s.show_non_manifold)),
                            (qa.open_edges, "Open edges", theme::MARK_OPEN, Some(&mut s.show_open_edges)),
                            (qa.overlapping_vertices, "Overlapping vertices", theme::MARK_OVERLAP, Some(&mut s.show_overlapping)),
                            (qa.inverted_normals, "Inverted normals", theme::MARK_NORMAL, Some(&mut s.show_normals)),
                            (qa.degenerate_faces, "Degenerate faces", theme::TEXT_FAINT, None),
                        ];
                        for (k, (count, label, color, toggle)) in rows.into_iter().enumerate() {
                            let width = ui.available_width();
                            let (rect, r) = ui.allocate_exact_size(vec2(width, 36.0), Sense::click());
                            if k > 0 {
                                ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme::BORDER));
                            }
                            let on = toggle.as_ref().is_some_and(|t| **t) && s.show_overlays;
                            if r.hovered() && toggle.is_some() {
                                ui.painter().rect_filled(rect.shrink(1.0), CornerRadius::ZERO, theme::WIDGET_HOVER);
                            }
                            let swatch = Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), Vec2::splat(10.0));
                            if on {
                                ui.painter().rect_filled(swatch, CornerRadius::same(2), color);
                            } else {
                                ui.painter().rect_stroke(swatch, CornerRadius::same(2), Stroke::new(1.5, color), egui::StrokeKind::Inside);
                            }
                            let text = if count > 0 { theme::TEXT } else { theme::TEXT_DIM };
                            ui.painter().text(pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, tr(label), FontId::proportional(13.0), text);
                            let value = if count > 0 { thousands(count) } else { "0".to_string() };
                            ui.painter().text(rect.right_center() - vec2(12.0, 0.0), Align2::RIGHT_CENTER, value, theme::mono(12.0), text);
                            if let Some(t) = toggle {
                                let r = r.on_hover_text(tr("Click to show them in the viewport"));
                                if r.clicked() {
                                    *t = !on;
                                    s.show_overlays |= *t;
                                }
                            }
                        }
                    });
                if qa.is_clean() {
                    ui.label(RichText::new(tr("No problems found: the mesh is closed and clean.")).size(11.0).color(theme::TEXT_DIM));
                }
            }
        }
        ui.add_space(12.0);

        let active = self.selection.active.map(|a| &info.objects[a]);
        let size = match active {
            Some(o) => o.bounds.size(),
            None => self.visible_bounds(false).size(),
        };
        widgets::section(ui, "Dimensions");
        table(ui, |t| {
            let labels = match self.settings.up_axis {
                crate::axes::UpAxis::Z => ["X, width", "Y, depth", "Z, height"],
                crate::axes::UpAxis::Y => ["X, width", "Y, height", "Z, depth"],
            };
            for (d, label) in labels.into_iter().enumerate() {
                t.row(label, &fmt_len(size[self.settings.up_axis.internal(d).0]));
            }
        });
        ui.add_space(12.0);

        if !self.manufacturing() {
            self.texel_rows(ui);
        }

        // Scale: declared units, where the scene origin sits, and a sanity check on size.
        let bounds = self.visible_bounds(false);
        let units = match info.units {
            crate::scene::Units::Meters => tr("Meters (glTF)").to_string(),
            crate::scene::Units::Undeclared if self.manufacturing() => tr("Not stored (mm)").to_string(),
            crate::scene::Units::Undeclared => tr("Not stored (m)").to_string(),
            crate::scene::Units::Declared(m) => unit_name(m, &info.path),
        };
        let floor = if !bounds.is_valid() {
            String::new()
        } else if bounds.min.z.abs() <= bounds.size().z.max(1e-6) * 0.01 {
            tr("On the floor").to_string()
        } else if bounds.min.z > 0.0 {
            trf("Raised +{d}", &[("d", &fmt_len(bounds.min.z))])
        } else {
            trf("Sunk −{d}", &[("d", &fmt_len(-bounds.min.z))])
        };
        let origin = tr(pivot_place(Vec3::ZERO, &bounds)).to_string();
        widgets::section(ui, "Scale");
        table(ui, |t| {
            t.row("File units", &units);
            t.row("World origin", &origin);
            t.row("Ground", &floor);
        });
        if let Some(note) = scale_note(bounds.size().max_element(), info.units) {
            ui.add(egui::Label::new(RichText::new(note).size(11.5).color(theme::TEXT_DIM)).wrap());
        }
        ui.add_space(12.0);

        if let Some(o) = active {
            widgets::section(ui, "Object");
            let material = &info.materials[o.material].name;
            let pivot = tr(pivot_place(o.origin, &o.bounds)).to_string();
            let shown = self.settings.show_origins && self.settings.show_overlays;
            let toggle = table(ui, |t| {
                t.row("Name", &o.name);
                t.row("Triangles", &thousands(o.triangles));
                t.row("Vertices", &thousands(o.vertices));
                t.row("Material", material);
                t.row_toggle("Pivot", &pivot, shown)
            });
            if toggle.on_hover_text(tr("Show pivots in the viewport, with the active object's axes")).clicked() {
                self.settings.show_origins = !shown;
                self.settings.show_overlays |= !shown;
            }
            ui.add_space(12.0);
        }

        let level = self.camera.view.distance.max(1e-6).log10() - GRID_LEVEL_OFFSET;
        let fine = 10f32.powf(level.floor());
        let cell = if level - level.floor() > 0.5 { fine * 10.0 } else { fine };
        widgets::section(ui, "View");
        let projection = tr(if self.camera.ortho { "Orthographic" } else { "Perspective" }).to_string();
        let load = trf("{ms} ms", &[("ms", &info.load_time.as_millis())]);
        table(ui, |t| {
            t.row("Grid", &fmt_len(cell));
            t.row("Projection", &projection);
            t.row("Opened in", &load);
        });
    }
}

/// Triangle budget: an editable target and a bar that turns amber near it, red past it.
fn budget_bar(ui: &mut Ui, budget: &mut usize, triangles: usize) {
    {
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr("Triangle budget")).size(12.0).color(theme::TEXT_DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(egui::DragValue::new(&mut *budget).speed(100.0).range(0..=100_000_000).custom_formatter(|v, _| {
                    if v == 0.0 { "—".to_string() } else { thousands(v as usize) }
                }))
                .on_hover_text(tr("Drag or type your target; 0 turns it off"));
            });
        });
        let budget = *budget;
        if budget == 0 {
            return;
        }
        let ratio = triangles as f32 / budget as f32;
        let color = if ratio > 1.0 { theme::ERROR } else if ratio > 0.9 { theme::MARK_OPEN } else { Color32::from_rgb(0x46, 0xa7, 0x58) };
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
        let bar = Rect::from_min_size(rect.min + vec2(0.0, 6.0), vec2(width - 56.0, 6.0));
        ui.painter().rect_filled(bar, CornerRadius::same(3), theme::WIDGET_HOVER);
        let fill = Rect::from_min_size(bar.min, vec2(bar.width() * ratio.min(1.0), bar.height()));
        ui.painter().rect_filled(fill, CornerRadius::same(3), color);
        let text = format!("{:.0}%", ratio * 100.0);
        ui.painter().text(pos2(rect.right(), bar.center().y), Align2::RIGHT_CENTER, text, theme::mono(11.0), color);
    }
}

impl ViewerApp {
    /// Texel density of the active object, or the range across the scene.
    fn texel_rows(&self, ui: &mut Ui) {
        let (Some(info), Some(uv)) = (&self.info, &self.uv) else { return };
        // Base color resolution of each object's material, else a 2K texture.
        let resolution = |object: usize| -> Option<(f64, bool)> {
            let material = &info.materials[info.objects[object].material];
            let base = material.maps.iter().find(|m| m.label.starts_with("Base Color"))?;
            let (_, w, h) = info.images.get(base.image)?;
            (*w > 0).then(|| (((*w as f64) * (*h as f64)).sqrt(), true))
        };
        let density = |object: usize| -> Option<(f64, bool)> {
            let mesh = uv.meshes.get(object)?.as_ref()?;
            let (size, real) = resolution(object).unwrap_or((2048.0, false));
            Some((mesh.texel_density(size)?, real))
        };
        let (value, note) = match self.selection.active {
            Some(a) => match density(a) {
                Some((d, real)) => (format!("{:.0} px/m", d), (!real).then(|| tr("No texture: assuming 2048 px").to_string())),
                None => (tr("No UVs").to_string(), None),
            },
            None => {
                let all: Vec<(f64, bool)> = (0..info.objects.len()).filter(|&i| self.visible[i]).filter_map(density).collect();
                if all.is_empty() {
                    (tr("No UVs").to_string(), None)
                } else {
                    let lo = all.iter().map(|d| d.0).fold(f64::INFINITY, f64::min);
                    let hi = all.iter().map(|d| d.0).fold(0.0, f64::max);
                    let text = if hi / lo < 1.05 { format!("{lo:.0} px/m") } else { format!("{lo:.0} – {hi:.0} px/m") };
                    let assumed = all.iter().any(|d| !d.1);
                    (text, assumed.then(|| tr("Objects without a texture assume 2048 px").to_string()))
                }
            }
        };
        widgets::section(ui, "Texture");
        table(ui, |t| t.row("Texel density", &crate::i18n::decimal(value)));
        if let Some(note) = note {
            ui.label(RichText::new(note).size(11.0).color(theme::TEXT_DIM));
        }
        ui.add_space(12.0);
    }

    fn materials_tab(&mut self, ui: &mut Ui) {
        let Some(info) = &self.info else { return };
        ui.spacing_mut().item_spacing.y = 12.0;
        let mut choose_folder = false;
        if !info.missing.is_empty() {
            Frame::new()
                .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
                .corner_radius(CornerRadius::same(theme::RADIUS))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 12.0;
                    ui.horizontal_top(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                        icons::warning(ui.painter(), r, theme::TEXT);
                        ui.add(egui::Label::new(RichText::new(trf(
                            "{n} textures weren't found next to the file. Point to the folder that has them.",
                            &[("n", &info.missing.len())],
                        )).size(14.0)).wrap());
                    });
                    let width = ui.available_width();
                    ui.allocate_ui(vec2(width, 40.0), |ui| {
                        ui.centered_and_justified(|ui| {
                            if primary_button(ui, "Choose texture folder…", 40.0, None).clicked() {
                                choose_folder = true;
                            }
                        });
                    });
                });
        }

        let mut show_pass = None;
        for (mi, material) in info.materials.iter().enumerate() {
            let used = info.objects.iter().any(|o| o.material == mi);
            if !used {
                continue;
            }
            widgets::section(ui, &material.name);
            let missing_here: Vec<&String> = material
                .maps
                .iter()
                .filter_map(|m| info.images.get(m.image).map(|i| &i.0))
                .filter(|n| info.missing.iter().any(|x| n.starts_with(x.as_str())))
                .collect();
            if material.maps.is_empty() && missing_here.is_empty() {
                ui.label(RichText::new(tr("No textures: plain material color.")).color(theme::TEXT_DIM));
                continue;
            }
            Frame::new()
                .stroke(Stroke::new(1.0, theme::BORDER))
                .corner_radius(CornerRadius::same(theme::RADIUS))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (k, m) in material.maps.iter().enumerate() {
                        let (name, w, h) = info.images.get(m.image).cloned().unwrap_or_default();
                        let texture = self.map_thumbs.get(&(m.image, m.channel)).map(|t| t.id());
                        let width = ui.available_width();
                        let (rect, r) = ui.allocate_exact_size(vec2(width, 60.0), Sense::click());
                        if k > 0 {
                            ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme::BORDER));
                        }
                        if r.hovered() {
                            ui.painter().rect_filled(rect.shrink(1.0), CornerRadius::ZERO, theme::WIDGET_HOVER);
                        }
                        let swatch = Rect::from_min_size(rect.min + vec2(12.0, 12.0), Vec2::splat(36.0));
                        match texture {
                            Some(t) => {
                                ui.painter().image(t, swatch, Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)), Color32::WHITE);
                            }
                            None => {
                                ui.painter().rect_filled(swatch, CornerRadius::same(2), theme::WIDGET_HOVER);
                                dashed_rect(ui.painter(), swatch, theme::BORDER_STRONG);
                            }
                        }
                        let x = swatch.right() + 12.0;
                        ui.painter().text(pos2(x, rect.top() + 20.0), Align2::LEFT_CENTER, m.label, FontId::proportional(14.0), theme::TEXT);
                        let detail = if w > 0 { format!("{name} · {w}×{h}") } else { name.clone() };
                        let clip = Rect::from_min_max(pos2(x, rect.top()), pos2(rect.right() - 12.0, rect.bottom()));
                        ui.painter().with_clip_rect(clip).text(pos2(x, rect.top() + 40.0), Align2::LEFT_CENTER, detail, theme::mono(11.0), theme::TEXT_DIM);
                        let r = r.on_hover_text(tr("Click to show this channel on the object (again to turn it off)"));
                        if r.clicked() {
                            show_pass = crate::ui::sidebar::pass_for(m.label);
                        }
                    }
                });
        }
        if !info.missing.is_empty() {
            widgets::section(ui, "Missing");
            table(ui, |t| {
                for name in &info.missing {
                    t.row(name, tr("MISSING"));
                }
            });
        }

        if let Some(pass) = show_pass {
            match self.selection.active {
                Some(a) => {
                    let current = self.pass_override[a];
                    self.pass_override[a] = if current == Some(pass) { None } else { Some(pass) };
                    if self.settings.shading == ShadingMode::Wireframe {
                        self.settings.shading = ShadingMode::Solid;
                    }
                }
                None => self.apply_channel(ChannelAction::Show(pass)),
            }
        }
        if choose_folder {
            self.choose_texture_folder(ui.ctx());
        }
    }

    fn scene_tab(&mut self, ui: &mut Ui) {
        let Some(info) = &self.info else { return };
        const ROW: f32 = 36.0;
        let width = ui.available_width();
        let (root, _) = ui.allocate_exact_size(vec2(width, ROW), Sense::hover());
        icons::chevron_small(ui.painter(), Rect::from_center_size(root.left_center() + vec2(15.0, 0.0), Vec2::splat(14.0)), theme::TEXT_DIM);
        ui.painter().text(root.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, &info.file_name, FontId::proportional(14.0), theme::TEXT_DIM);

        let states = self.selection.states();
        let mut actions = Vec::new();
        ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW + 2.0, info.objects.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for i in range {
                let o = &info.objects[i];
                let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                let painter = ui.painter();
                let state = states[i];
                let fill = match state {
                    2 => theme::WIDGET_PRESS,
                    1 => theme::WIDGET_HOVER,
                    _ if r.hovered() => theme::SURFACE,
                    _ => Color32::TRANSPARENT,
                };
                painter.rect_filled(rect, CornerRadius::same(theme::RADIUS), fill);
                if state == 2 {
                    // The active object keeps Blender's orange as a thin marker.
                    painter.rect_filled(Rect::from_min_size(rect.min + vec2(0.0, 8.0), vec2(2.0, ROW - 16.0)), CornerRadius::same(1), theme::SELECTION);
                }
                let visible = self.visible[i];
                let text = if !visible { theme::TEXT_FAINT } else if state > 0 { theme::TEXT } else { theme::TEXT_DIM };
                icons::cube_line(painter, Rect::from_center_size(rect.left_center() + vec2(37.0, 0.0), Vec2::splat(14.0)), text);
                let eye_rect = Rect::from_center_size(rect.right_center() - vec2(16.0, 0.0), vec2(24.0, ROW));
                let right = if let Some(pass) = self.pass_override[i] {
                    let g = painter.layout_job(theme::caps(pass.label(), 10.0, theme::ON_ACCENT));
                    let badge = Rect::from_min_size(pos2(eye_rect.left() - g.size().x - 14.0, rect.center().y - 8.0), vec2(g.size().x + 8.0, 16.0));
                    painter.rect_filled(badge, CornerRadius::same(2), theme::ACCENT);
                    painter.galley(badge.center() - g.size() * 0.5, g, theme::ON_ACCENT);
                    badge.left() - 8.0
                } else {
                    let g = painter.layout_no_wrap(trf("{n} tri", &[("n", &thousands(o.triangles))]), theme::mono(11.0), theme::TEXT_DIM);
                    let x = eye_rect.left() - 6.0 - g.size().x;
                    painter.galley(pos2(x, rect.center().y - g.size().y * 0.5), g, theme::TEXT_DIM);
                    x - 8.0
                };
                let clip = Rect::from_min_max(pos2(rect.left() + 52.0, rect.top()), pos2(right, rect.bottom()));
                painter.with_clip_rect(clip).text(clip.left_center(), Align2::LEFT_CENTER, &o.name, FontId::proportional(14.0), text);

                let eye = ui.interact(eye_rect, ui.id().with(("eye", i)), Sense::click());
                icons::eye(ui.painter(), eye_rect.shrink(4.0), if eye.hovered() { theme::TEXT } else { theme::TEXT_DIM }, visible);
                if eye.on_hover_text(tr(if visible { "Hide (H)" } else { "Show (Alt H)" })).clicked() {
                    actions.push(SidebarAction::ToggleVisible(i));
                    continue;
                }
                if r.double_clicked() {
                    actions.push(SidebarAction::Frame(i));
                } else if r.clicked() {
                    let extend = ui.input(|i| i.modifiers.shift || i.modifiers.ctrl);
                    actions.push(SidebarAction::Select { index: i, extend });
                }
                r.on_hover_text(trf(
                    "{name}\nClick to select · Shift/Ctrl to extend · Double-click to frame",
                    &[("name", &o.name)],
                ));
            }
        });
        for action in actions {
            self.sidebar_action(action);
        }
    }

    fn sidebar_action(&mut self, action: SidebarAction) {
        match action {
            SidebarAction::Select { index, extend } => {
                if extend && self.selection.selected[index] {
                    // Outliner-style toggle when extending.
                    self.selection.selected[index] = false;
                    if self.selection.active == Some(index) {
                        self.selection.active = None;
                    }
                } else {
                    self.selection.click(Some(index), extend);
                }
            }
            SidebarAction::ToggleVisible(i) => {
                self.visible[i] = !self.visible[i];
                if !self.visible[i] {
                    self.selection.selected[i] = false;
                    if self.selection.active == Some(i) {
                        self.selection.active = None;
                    }
                }
            }
            SidebarAction::Frame(i) => {
                let b = self.info.as_ref().map(|info| info.objects[i].bounds);
                if let Some(b) = b {
                    self.selection.click(Some(i), false);
                    self.camera.frame(&b, true);
                }
            }
            SidebarAction::ShowPass(_) => {}
        }
    }

    // --- Footer ----------------------------------------------------------------------------------

    pub(super) fn footer(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if self.info.is_some() && self.tool == Tool::Section {
                widgets::hint(ui, &["LMB"], "Drag the plane");
                for (keys, action) in self.settings.navigation.hints() {
                    widgets::hint(ui, keys, action);
                }
                widgets::hint(ui, &["Q"], "Back to select");
            } else if self.info.is_some() && self.tool.transforms() {
                if self.lay_face_armed {
                    widgets::hint(ui, &["LMB"], "Face to lay on the bed");
                    widgets::hint(ui, &["Esc"], "Cancel");
                } else {
                    widgets::hint(ui, &["LMB"], match self.tool {
                        Tool::Rotate => "Drag a ring",
                        Tool::Scale => "Drag a handle, or the center for all axes",
                        _ => "Drag an arrow, or the center",
                    });
                    widgets::hint(ui, &["Ctrl"], "Snap");
                    widgets::hint(ui, &["L"], "Lay on face");
                    widgets::hint(ui, &["B"], "Drop to bed");
                    widgets::hint(ui, &["Ctrl", "Z"], "Undo");
                }
                widgets::hint(ui, &["Q"], "Back to select");
            } else if self.info.is_some() && self.tool == Tool::Measure {
                widgets::hint(ui, &["LMB"], if self.measure_start.is_some() { "Second point" } else { "First point" });
                if self.measure_start.is_some() {
                    widgets::hint(ui, &["Esc"], "Cancel point");
                }
                if !self.measures.is_empty() {
                    widgets::hint(ui, &["Del"], "Remove last");
                    widgets::hint(ui, &["Shift", "Del"], "Clear all");
                }
                widgets::hint(ui, &["Ctrl"], "No vertex snap");
                widgets::hint(ui, &["Q"], "Back to select");
            } else if self.info.is_some() {
                widgets::hint(ui, &["LMB"], "Select");
                for (keys, action) in self.settings.navigation.hints() {
                    widgets::hint(ui, keys, action);
                }
                widgets::hint(ui, &["Z"], "Shading");
                widgets::hint(ui, &["N"], "Inspector");
            } else {
                ui.label(theme::caps(tr("No file open"), 11.0, theme::TEXT_DIM));
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut parts = Vec::new();
                if let Some(info) = &self.info {
                    parts.push(trf("{tris} tris", &[("tris", &thousands(info.triangles))]));
                    parts.push(trf("opened in {ms} ms", &[("ms", &info.load_time.as_millis())]));
                }
                if self.settings.show_fps {
                    parts.push(format!("{:.0} fps · {:.2} ms", self.fps, 1000.0 / self.fps.max(1e-3)));
                }
                parts.push(format!("v{}", env!("CARGO_PKG_VERSION")));
                ui.label(theme::caps(&parts.join(" · "), 11.0, theme::TEXT_DIM));
                if self.env_loading.is_some() {
                    ui.add_space(8.0);
                    ui.label(theme::caps(tr("Preparing lighting…"), 11.0, theme::TEXT_DIM));
                    ui.add(egui::Spinner::new().size(11.0).color(theme::TEXT_DIM));
                }
            });
        });
    }

    // --- Empty state, banners, overlays ------------------------------------------------------------

    pub(super) fn empty_state(&mut self, ui: &mut Ui, viewport: Rect) {
        let painter = ui.painter();
        painter.rect_filled(viewport, 0.0, Color32::from_rgb(0x0a, 0x0a, 0x0a));
        let step = 24.0;
        let mut y = viewport.top() + step * 0.5;
        while y < viewport.bottom() {
            let mut x = viewport.left() + step * 0.5;
            while x < viewport.right() {
                painter.circle_filled(pos2(x, y), 1.0, theme::BORDER);
                x += step;
            }
            y += step;
        }

        let ctx = ui.ctx().clone();
        let recent: Vec<String> = self.settings.recent_files.iter().take(5).cloned().collect();
        let card_h = 330.0;
        let list_h = if recent.is_empty() { 0.0 } else { 32.0 + 56.0 * recent.len() as f32 };
        let total = vec2(520.0, card_h + list_h);
        let area = Rect::from_center_size(viewport.center(), total);
        let mut open_path = None;
        let mut open_dialog = false;
        ui.scope_builder(UiBuilder::new().max_rect(area).layout(Layout::top_down(Align::Center)), |ui| {
            let (card, _) = ui.allocate_exact_size(vec2(520.0, card_h), Sense::hover());
            ui.painter().rect_filled(card, CornerRadius::same(theme::RADIUS), theme::BG_APP);
            dashed_rect(ui.painter(), card, theme::BORDER_STRONG);
            ui.scope_builder(UiBuilder::new().max_rect(card.shrink2(vec2(32.0, 44.0))).layout(Layout::top_down(Align::Center)), |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                if let Some(logo) = &self.logo {
                    ui.add(Image::new(logo).fit_to_exact_size(vec2(72.0, 72.0)).corner_radius(16));
                }
                ui.add_space(14.0);
                ui.label(RichText::new(tr("Drop a model here")).font(theme::bold(28.0)).color(theme::TEXT));
                ui.label(
                    RichText::new(tr("or open it from disk. Textures are looked up in the same folder."))
                        .size(15.0)
                        .color(theme::TEXT_DIM),
                );
                ui.add_space(14.0);
                if primary_button(ui, "Open File…", 44.0, Some("Ctrl+O")).clicked() {
                    open_dialog = true;
                }
                ui.add_space(12.0);
                ui.label(theme::caps("GLB · GLTF · FBX · OBJ · STL · HDR · EXR", 11.0, theme::TEXT_DIM));
            });
            if !recent.is_empty() {
                let list = Rect::from_min_size(card.left_bottom() + vec2(0.0, 24.0), vec2(520.0, list_h));
                ui.scope_builder(UiBuilder::new().max_rect(list).layout(Layout::top_down(Align::Min)), |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label(theme::caps(tr("Recent"), 11.0, theme::TEXT_DIM));
                    for path in &recent {
                        let (rect, r) = ui.allocate_exact_size(vec2(520.0, 48.0), Sense::click());
                        let p = ui.painter();
                        p.rect(rect, CornerRadius::same(theme::RADIUS), if r.hovered() { theme::WIDGET_HOVER } else { theme::BG_APP }, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
                        icons::cube_line(p, Rect::from_center_size(rect.left_center() + vec2(21.0, 0.0), Vec2::splat(18.0)), theme::TEXT);
                        p.text(rect.left_center() + vec2(42.0, 0.0), Align2::LEFT_CENTER, file_name(Path::new(path)), theme::medium(14.0), theme::TEXT);
                        let folder = Path::new(path).parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
                        p.text(rect.right_center() - vec2(12.0, 0.0), Align2::RIGHT_CENTER, elide_start(&folder, 40), theme::mono(11.0), theme::TEXT_DIM);
                        if r.on_hover_text(path).clicked() {
                            open_path = Some(PathBuf::from(path));
                        }
                    }
                });
            }
        });
        if open_dialog {
            self.open_dialog(&ctx);
        }
        if let Some(path) = open_path {
            self.open(path, &ctx);
        }
    }

    /// Bottom banner: icon, text, optional action, dismiss.
    pub(super) fn banner(&mut self, ctx: &egui::Context) {
        let Some(toast) = &self.toast else { return };
        if toast.action.is_none() && ctx.input(|i| i.time) > toast.until {
            self.toast = None;
            return;
        }
        let mut close = false;
        let mut act = None;
        let anchor = self.viewport_rect.center_bottom() - vec2(0.0, 20.0);
        egui::Area::new(Id::new("banner"))
            .order(Order::Foreground)
            .fixed_pos(anchor)
            .pivot(Align2::CENTER_BOTTOM)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(Stroke::new(1.0, if toast.error { theme::ERROR } else { theme::BORDER_STRONG }))
                    .corner_radius(CornerRadius::same(theme::RADIUS))
                    .inner_margin(Margin { left: 14, right: 8, top: 8, bottom: 8 })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                            icons::warning(ui.painter(), r, if toast.error { theme::ERROR } else { theme::TEXT });
                            ui.label(RichText::new(&toast.text).color(theme::TEXT));
                            if let Some(a) = toast.action {
                                if primary_button(ui, if a == ToastAction::OpenReleases { "Download" } else { "Fix" }, 32.0, None).clicked() {
                                    act = Some(a);
                                }
                            }
                            if widgets::sized_icon_button(ui, icons::close, false, Vec2::splat(32.0)).on_hover_text(tr("Dismiss")).clicked() {
                                close = true;
                            }
                        });
                    });
            });
        match act {
            Some(ToastAction::ShowMaterials) => {
                self.settings.show_sidebar = true;
                self.inspector_tab = InspectorTab::Materials;
                close = true;
            }
            Some(ToastAction::OpenReleases) => {
                let _ = std::process::Command::new("explorer.exe").arg(format!("{REPO_URL}/releases/latest")).spawn();
                close = true;
            }
            None => {}
        }
        if close {
            self.toast = None;
        }
    }

    /// Reloads the current model, also looking for its textures in a folder the user picks.
    pub(super) fn choose_texture_folder(&mut self, ctx: &egui::Context) {
        let Some(info) = &self.info else { return };
        let start = info.path.parent().map(Path::to_path_buf);
        let mut dialog = rfd::FileDialog::new().set_title(tr("Choose texture folder"));
        if let Some(dir) = start {
            dialog = dialog.set_directory(dir);
        }
        if let Some(dir) = dialog.pick_folder() {
            if !self.texture_dirs.contains(&dir) {
                self.texture_dirs.push(dir);
            }
            let path = info.path.clone();
            self.open(path, ctx);
        }
    }
}

/// Keeps the end of a long path: "…\GitHub\project\assets".
/// Where a pivot sits relative to bounds: the spots game engines and DCCs care about.
fn pivot_place(p: Vec3, b: &Aabb) -> &'static str {
    if !b.is_valid() {
        return "Unknown";
    }
    let size = b.size().max(Vec3::splat(1e-6));
    let rel = (p - b.min) / size;
    if rel.min_element() < -0.1 || rel.max_element() > 1.1 {
        return "Outside the model";
    }
    let centered = (rel.x - 0.5).abs() < 0.1 && (rel.y - 0.5).abs() < 0.1;
    match (centered, rel.z) {
        (true, z) if z < 0.1 => "Bottom center",
        (true, z) if (z - 0.5).abs() < 0.1 => "Center",
        (true, z) if z > 0.9 => "Top center",
        _ => "Off center",
    }
}

fn unit_name(meters: f64, path: &Path) -> String {
    let format = path.extension().map_or(String::new(), |e| e.to_string_lossy().to_uppercase());
    let known = [(1.0, "Meters"), (0.01, "Centimeters"), (0.001, "Millimeters"), (0.0254, "Inches"), (0.3048, "Feet"), (1000.0, "Kilometers")];
    match known.iter().find(|(m, _)| (meters / m - 1.0).abs() < 1e-3) {
        Some((_, name)) => format!("{} ({format})", tr(name)),
        None => format!("{} m ({format})", crate::i18n::decimal(format!("{meters}"))),
    }
}

/// A hint when the size looks like a units mix-up (the classic 100x FBX).
fn scale_note(largest: f32, units: crate::scene::Units) -> Option<String> {
    // Manufacturing reads unitless files as millimeters already.
    if !(largest > 0.0) || meters_per_unit() != 1.0 {
        return None;
    }
    if largest > 100.0 {
        return Some(trf(
            "{size} across: huge for a single asset. If it was modeled in centimeters and read as meters, it's 100× too big ({real}).",
            &[("size", &fmt_len(largest)), ("real", &fmt_len(largest / 100.0))],
        ));
    }
    if largest < 0.005 {
        return Some(trf(
            "{size} across: tiny. If it was modeled in meters and exported as millimeters, it's 1000× too small ({real}).",
            &[("size", &fmt_len(largest)), ("real", &fmt_len(largest * 1000.0))],
        ));
    }
    if units == crate::scene::Units::Undeclared && largest > 10.0 {
        return Some(trf(
            "This format doesn't store units. If the author worked in millimeters (common for STL), the real size is {real}.",
            &[("real", &fmt_len(largest / 1000.0))],
        ));
    }
    None
}

fn elide_start(text: &str, max_chars: usize) -> String {
    let count = text.chars().count();
    if count <= max_chars {
        return text.to_string();
    }
    let tail: String = text.chars().skip(count - (max_chars - 1)).collect();
    format!("…{tail}")
}

fn window_button(ui: &mut Ui, icon: icons::IconFn, size: Vec2, close: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    let hovered = r.hovered();
    let fill = match (hovered, close) {
        (true, true) => Color32::from_rgb(0xc4, 0x2b, 0x1c),
        (true, false) => theme::WIDGET_HOVER,
        _ => Color32::TRANSPARENT,
    };
    ui.painter().rect_filled(rect, CornerRadius::same(theme::RADIUS), fill);
    icon(ui.painter(), Rect::from_center_size(rect.center(), Vec2::splat(12.0)), if hovered { theme::TEXT } else { theme::TEXT_DIM });
    r
}

pub(super) fn loading_overlay(ui: &mut Ui, viewport: Rect, name: &str) {
    let card = Rect::from_center_size(viewport.center(), vec2(340.0, 76.0));
    ui.painter().rect(card, CornerRadius::same(theme::RADIUS), theme::SURFACE, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
    ui.scope_builder(UiBuilder::new().max_rect(card.shrink(12.0)).layout(Layout::top_down(Align::Center)), |ui| {
        ui.add(egui::Spinner::new().size(20.0).color(theme::TEXT));
        ui.add_space(4.0);
        ui.label(RichText::new(trf("Opening {name}…", &[("name", &name)])).color(theme::TEXT));
    });
}

/// With a model open, the right half takes the drop as model B for an A/B comparison.
pub(super) fn drop_overlay(ui: &Ui, viewport: Rect, has_model: bool) {
    let painter = ui.painter();
    painter.rect_filled(viewport, 0.0, Color32::from_black_alpha(150));
    if !has_model {
        dashed_rect(painter, viewport.shrink(16.0), theme::TEXT);
        painter.text(viewport.center(), Align2::CENTER_CENTER, tr("Drop to open"), theme::bold(22.0), theme::TEXT);
        return;
    }
    let pointer = ui.ctx().input(|i| i.pointer.hover_pos());
    let (left, right) = viewport.split_left_right_at_x(viewport.center().x);
    for (zone, title, sub) in [(left, "Drop to open", "Replaces the model"), (right, "Drop to compare", "Opens it as B, next to A")] {
        let hot = pointer.is_some_and(|p| zone.contains(p));
        if hot {
            painter.rect_filled(zone.shrink(16.0), CornerRadius::same(theme::RADIUS), Color32::from_white_alpha(14));
        }
        dashed_rect(painter, zone.shrink(16.0), if hot { theme::TEXT } else { theme::TEXT_DIM });
        painter.text(zone.center() - vec2(0.0, 12.0), Align2::CENTER_CENTER, tr(title), theme::bold(22.0), theme::TEXT);
        painter.text(zone.center() + vec2(0.0, 16.0), Align2::CENTER_CENTER, tr(sub), FontId::proportional(13.0), theme::TEXT_DIM);
    }
}

/// Elevation range a light can take, in degrees: the key light (the shadow caster) stays above
/// the horizon, the others may light from below like the Render tab's slider allows.
pub(super) fn light_pitch_range(index: usize) -> (f32, f32) {
    if index == 0 { (5.0, 85.0) } else { (-60.0, 85.0) }
}
