//! Move tool: a Blender-style transform gizmo (arrows move along an axis, rings rotate around
//! it, the center dot moves in the view plane), laying an object on a face for printing, and
//! exporting the moved model as STL or 3MF.
//!
//! Each object keeps a user transform applied on top of the file's placement. The file itself is
//! never touched; File > Export Model writes a new one.

use std::f32::consts::TAU;
use std::io::Write;

use eframe::egui::{Id, Order};

use super::*;
use crate::ui::widgets::{text_button, toolbar, toolbar_separator};

/// Gizmo arrow length and ring radius, in points.
const ARROW_LEN: f32 = 90.0;
const RING_RADIUS: f32 = 62.0;
const CENTER_RADIUS: f32 = 9.0;
/// How close (points) the pointer must be to grab a handle.
const GRAB: f32 = 7.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Handle {
    /// Move along world axis 0..2.
    Axis(usize),
    /// Rotate around world axis 0..2.
    Ring(usize),
    /// Move in the view plane.
    Center,
}

pub(super) struct Drag {
    handle: Handle,
    pivot: Vec3,
    /// User transforms when the drag started (undo snapshot).
    start: Vec<Mat4>,
    start_pointer: Pos2,
    /// Angle accumulated by a ring drag, unwrapped across ±π.
    angle: f32,
    last_angle: f32,
}

/// The gizmo as seen this frame.
struct Geometry {
    pivot: Vec3,
    center: Pos2,
    /// World units per point at the pivot.
    world_per_point: f32,
    arrows: [Option<Pos2>; 3],
    rings: [Vec<Pos2>; 3],
}

impl ViewerApp {
    /// Objects the Move tool acts on: the visible selection, or with nothing selected the whole
    /// visible model (a part made of several solids moves as one).
    pub(super) fn transform_targets(&self) -> Vec<usize> {
        let Some(info) = &self.info else { return Vec::new() };
        let visible = |i: usize| self.visible.get(i).copied().unwrap_or(true);
        let selected: Vec<usize> = (0..info.objects.len()).filter(|&i| self.selection.selected.get(i).copied().unwrap_or(false) && visible(i)).collect();
        if selected.is_empty() { (0..info.objects.len()).filter(|&i| visible(i)).collect() } else { selected }
    }

    fn targets_bounds(&self, targets: &[usize]) -> Aabb {
        let mut b = Aabb::EMPTY;
        if let Some(info) = &self.info {
            for &i in targets {
                b.union(&info.objects[i].bounds);
            }
        }
        b
    }

    /// Applies the user transforms to everything derived from object placement. `exact`
    /// measures the moved vertices (end of a drag); otherwise bounds come from the rest box.
    pub(super) fn transforms_changed(&mut self, exact: bool) {
        let Some(info) = &mut self.info else { return };
        for (i, o) in info.objects.iter_mut().enumerate() {
            let user = self.user_transforms.get(i).copied().unwrap_or(Mat4::IDENTITY);
            if let Some(snap) = self.snap.get_mut(i) {
                snap.place(user);
            }
            o.bounds = match self.snap.get(i) {
                Some(snap) if exact => snap.world_bounds(),
                _ => o.rest_bounds.transformed(&user),
            };
            o.origin = user.transform_point3(o.rest_origin);
            o.axes = o.rest_axes.map(|a| user.transform_vector3(a).normalize_or_zero());
        }
    }

    fn push_transform_undo(&mut self, snapshot: Vec<Mat4>) {
        self.transform_undo.push(snapshot);
        if self.transform_undo.len() > 64 {
            self.transform_undo.remove(0);
        }
    }

    /// Ctrl+Z for the Move tool.
    pub(super) fn undo_transform(&mut self) -> bool {
        let Some(previous) = self.transform_undo.pop() else { return false };
        self.user_transforms = previous;
        self.transforms_changed(true);
        true
    }

    /// Replaces the user transform of `targets` with `f(old)`, as one undo step.
    fn edit_transforms(&mut self, targets: &[usize], f: impl Fn(usize, Mat4) -> Mat4) {
        if targets.is_empty() {
            return;
        }
        self.push_transform_undo(self.user_transforms.clone());
        for &i in targets {
            self.user_transforms[i] = f(i, self.user_transforms[i]);
        }
        self.transforms_changed(true);
    }

    /// Moves `targets` straight down (or up) until their lowest point touches Z = 0.
    fn drop_to_bed(&mut self, targets: &[usize], undo: bool) {
        let Some(info) = &self.info else { return };
        let lifts: Vec<(usize, f32)> = targets.iter().map(|&i| (i, -info.objects[i].bounds.min.z)).filter(|(_, d)| d.is_finite()).collect();
        if undo {
            self.push_transform_undo(self.user_transforms.clone());
        }
        for (i, lift) in lifts {
            self.user_transforms[i] = Mat4::from_translation(Vec3::Z * lift) * self.user_transforms[i];
        }
        self.transforms_changed(true);
    }

    pub(super) fn auto_orient(&mut self, ctx: &egui::Context) {
        let targets = self.transform_targets();
        if self.snap.is_empty() {
            self.show_toast(ctx, tr("Still analyzing the model, try again in a moment").to_string(), false);
            return;
        }
        // The targets turn as one, so an assembly stays assembled.
        let meshes: Vec<&snap::SnapMesh> = targets.iter().filter_map(|&i| self.snap.get(i)).collect();
        let Some(down) = snap::best_down_direction(&meshes) else {
            self.show_toast(ctx, tr("No flat face to lay the model on").to_string(), false);
            return;
        };
        self.push_transform_undo(self.user_transforms.clone());
        self.lay_group(&targets, down);
    }

    /// Lay on face: the clicked face goes down on the bed. Every object the Move tool acts on
    /// turns with it when the clicked one is among them, so a multi-part model stays together.
    pub(super) fn lay_on_picked_face(&mut self, ctx: &egui::Context, object: Option<usize>, point: Option<Vec3>) {
        let (Some(i), Some(p)) = (object, point) else { return };
        let Some(normal) = self.snap.get(i).and_then(|s| s.face_normal(p)) else {
            self.show_toast(ctx, tr("Still analyzing the model, try again in a moment").to_string(), false);
            return;
        };
        self.lay_face_armed = false;
        self.lay_hover = None;
        let targets = self.transform_targets();
        let group = if targets.contains(&i) { targets } else { vec![i] };
        self.push_transform_undo(self.user_transforms.clone());
        self.lay_group(&group, normal);
    }

    /// Like `lay_down`, for several objects turning together around their common center.
    fn lay_group(&mut self, group: &[usize], down: Vec3) {
        let c = self.targets_bounds(group).center();
        let rotation = glam::Quat::from_rotation_arc(down.normalize(), Vec3::NEG_Z);
        let m = Mat4::from_translation(c) * Mat4::from_quat(rotation) * Mat4::from_translation(-c);
        for &i in group {
            self.user_transforms[i] = m * self.user_transforms[i];
        }
        self.transforms_changed(true);
        // Down as one: the group's lowest point touches the bed.
        let lift = -self.targets_bounds(group).min.z;
        if lift.is_finite() {
            for &i in group {
                self.user_transforms[i] = Mat4::from_translation(Vec3::Z * lift) * self.user_transforms[i];
            }
            self.transforms_changed(true);
        }
    }

    /// Lay on face, while choosing: the face under the cursor (surface point and outward normal).
    pub(super) fn lay_hover_at(&mut self, object: Option<usize>, point: Option<Vec3>) {
        self.lay_hover = match (object, point) {
            (Some(i), Some(p)) => self.snap.get(i).and_then(|s| s.face_normal(p)).map(|n| (p, n)),
            _ => None,
        };
    }

    pub(super) fn reset_transforms(&mut self) {
        let targets = self.transform_targets();
        self.edit_transforms(&targets, |_, _| Mat4::IDENTITY);
    }

    // --- Gizmo ---------------------------------------------------------------------------------

    fn gizmo_geometry(&self, viewport: Rect) -> Option<Geometry> {
        let targets = self.transform_targets();
        let bounds = self.targets_bounds(&targets);
        if !bounds.is_valid() {
            return None;
        }
        let pivot = bounds.center();
        let aspect = viewport.width() / viewport.height().max(1.0);
        let view_proj = self.camera.projection(aspect) * self.camera.view_matrix();
        let project = |p: Vec3| {
            let c = view_proj * p.extend(1.0);
            (c.w > 1e-6).then(|| {
                let n = c.truncate() / c.w;
                pos2(viewport.left() + (n.x * 0.5 + 0.5) * viewport.width(), viewport.top() + (0.5 - n.y * 0.5) * viewport.height())
            })
        };
        let center = project(pivot)?;
        // Screen length of one world unit along the camera's right axis, at the pivot.
        let unit = project(pivot + self.camera.right())?.distance(center);
        if unit <= 1e-9 {
            return None;
        }
        let world_per_point = 1.0 / unit;
        let axes = [Vec3::X, Vec3::Y, Vec3::Z];
        let arrows = axes.map(|a| project(pivot + a * ARROW_LEN * world_per_point));
        let rings = axes.map(|a| {
            let (u, v) = a.any_orthonormal_pair();
            (0..=64)
                .filter_map(|k| {
                    let t = k as f32 / 64.0 * TAU;
                    project(pivot + (u * t.cos() + v * t.sin()) * RING_RADIUS * world_per_point)
                })
                .collect()
        });
        Some(Geometry { pivot, center, world_per_point, arrows, rings })
    }

    fn hovered_handle(geo: &Geometry, pointer: Pos2) -> Option<Handle> {
        if pointer.distance(geo.center) <= CENTER_RADIUS + 2.0 {
            return Some(Handle::Center);
        }
        let mut best: Option<(f32, Handle)> = None;
        let mut consider = |d: f32, h: Handle| {
            if d <= GRAB && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, h));
            }
        };
        for k in 0..3 {
            if let Some(tip) = geo.arrows[k] {
                // Skip axes pointing at the viewer: they collapse onto the center.
                if tip.distance(geo.center) > 12.0 {
                    consider(segment_distance(pointer, geo.center, tip), Handle::Axis(k));
                }
            }
            for w in geo.rings[k].windows(2) {
                consider(segment_distance(pointer, w[0], w[1]), Handle::Ring(k));
            }
        }
        best.map(|(_, h)| h)
    }

    /// Gizmo input, before navigation. Returns true when the pointer belongs to the gizmo, so
    /// the click doesn't select.
    pub(super) fn transform_input(&mut self, ui: &Ui, response: &egui::Response, viewport: Rect) -> bool {
        if self.tool != Tool::Move || self.lay_face_armed {
            self.transform_drag = None;
            return false;
        }
        let Some(geo) = self.gizmo_geometry(viewport) else {
            self.transform_drag = None;
            return false;
        };
        let (pointer, mods) = ui.input(|i| (i.pointer.interact_pos().or(i.pointer.hover_pos()), i.modifiers));
        if mods.alt {
            return false;
        }
        let hovered = pointer.and_then(|p| Self::hovered_handle(&geo, p));
        if response.drag_started_by(PointerButton::Primary) {
            let start_pointer = response.interact_pointer_pos().unwrap_or(geo.center);
            if let Some(handle) = Self::hovered_handle(&geo, start_pointer) {
                let a = (start_pointer - geo.center).angle();
                self.transform_drag = Some(Drag { handle, pivot: geo.pivot, start: self.user_transforms.clone(), start_pointer, angle: 0.0, last_angle: a });
            }
        }
        let Some(mut drag) = self.transform_drag.take() else {
            if hovered.is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            return hovered.is_some() && (response.clicked() || response.drag_started());
        };
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let now = pointer.unwrap_or(drag.start_pointer);
        let targets = self.transform_targets();
        let snap = mods.ctrl;
        let m = match drag.handle {
            Handle::Axis(k) => {
                let Some(tip) = geo.arrows[k] else { return true };
                let axis_screen = tip - geo.center;
                let along = (now - drag.start_pointer).dot(axis_screen) / axis_screen.length_sq().max(1e-6);
                let mut distance = along * ARROW_LEN * geo.world_per_point;
                if snap {
                    let cell = grid_params(&self.camera).0;
                    distance = (distance / cell).round() * cell;
                }
                let mut d = Vec3::ZERO;
                d[k] = distance;
                Mat4::from_translation(d)
            }
            Handle::Center => {
                let delta = (now - drag.start_pointer) * geo.world_per_point;
                Mat4::from_translation(self.camera.right() * delta.x - self.camera.up() * delta.y)
            }
            Handle::Ring(k) => {
                let a = (now - geo.center).angle();
                let mut step = a - drag.last_angle;
                if step > std::f32::consts::PI {
                    step -= TAU;
                } else if step < -std::f32::consts::PI {
                    step += TAU;
                }
                drag.angle += step;
                drag.last_angle = a;
                let mut axis = Vec3::ZERO;
                axis[k] = 1.0;
                // Screen angles grow clockwise (y down); seen from the axis tip that's negative.
                let facing = axis.dot(self.camera.back()) >= 0.0;
                let mut angle = if facing { -drag.angle } else { drag.angle };
                if snap {
                    let step = 15f32.to_radians();
                    angle = (angle / step).round() * step;
                }
                Mat4::from_translation(drag.pivot) * Mat4::from_axis_angle(axis, angle) * Mat4::from_translation(-drag.pivot)
            }
        };
        for &i in &targets {
            self.user_transforms[i] = m * drag.start[i];
        }
        let released = !response.dragged_by(PointerButton::Primary);
        if released {
            self.push_transform_undo(drag.start);
            self.transforms_changed(true);
        } else {
            self.transforms_changed(false);
            self.transform_drag = Some(drag);
        }
        true
    }

    /// Lay on face, while choosing: what to do, and the face under the cursor as a disc lying on
    /// it with an arrow pointing where it will go (down, onto the bed).
    fn draw_lay_preview(&self, ui: &Ui, viewport: Rect) {
        let painter = ui.painter().with_clip_rect(viewport);
        let shadow = Color32::from_black_alpha(160);
        // Instruction under the toolbar, where the eye already is.
        let text = tr("Click the face that goes on the bed · Esc to cancel");
        let galley = painter.layout_no_wrap(text.to_string(), theme::medium(13.0), theme::ON_ACCENT);
        let top = self.toolbar_rect.bottom().max(viewport.top()) + 10.0;
        let chip = Rect::from_center_size(pos2(viewport.center().x, top + galley.size().y * 0.5 + 7.0), galley.size() + vec2(24.0, 14.0));
        painter.rect_filled(chip, CornerRadius::same(theme::RADIUS), theme::ACCENT);
        painter.galley(chip.min + vec2(12.0, 7.0), galley, theme::ON_ACCENT);

        let Some((p, n)) = self.lay_hover else { return };
        let aspect = viewport.width() / viewport.height().max(1.0);
        let view_proj = self.camera.projection(aspect) * self.camera.view_matrix();
        let project = |w: Vec3| {
            let c = view_proj * w.extend(1.0);
            (c.w > 1e-6).then(|| {
                let q = c.truncate() / c.w;
                pos2(viewport.left() + (q.x * 0.5 + 0.5) * viewport.width(), viewport.top() + (0.5 - q.y * 0.5) * viewport.height())
            })
        };
        let (Some(at), Some(side)) = (project(p), project(p + self.camera.right())) else { return };
        let world_per_point = 1.0 / at.distance(side).max(1e-6);
        let radius = 34.0 * world_per_point;
        let (u, v) = n.any_orthonormal_pair();
        let lift = n * radius * 0.02;
        let disc: Vec<Pos2> = (0..48)
            .filter_map(|k| {
                let t = k as f32 / 48.0 * TAU;
                project(p + lift + (u * t.cos() + v * t.sin()) * radius)
            })
            .collect();
        if disc.len() == 48 {
            painter.add(egui::Shape::convex_polygon(disc.clone(), theme::ACCENT.gamma_multiply(0.35), Stroke::NONE));
            let mut ring = disc;
            ring.push(ring[0]);
            painter.add(egui::Shape::line(ring.clone(), Stroke::new(4.0, shadow)));
            painter.add(egui::Shape::line(ring, Stroke::new(2.0, theme::ACCENT)));
        }
        // An arrow pressing on the face: this side goes down.
        if let Some(tail) = project(p + n * radius * 1.8) {
            let dir = (at - tail).normalized();
            let tip = at - dir * 4.0;
            painter.line_segment([tail, tip], Stroke::new(4.5, shadow));
            painter.line_segment([tail, tip], Stroke::new(2.2, theme::ACCENT));
            let side = vec2(-dir.y, dir.x);
            painter.add(egui::Shape::convex_polygon(vec![tip, tip - dir * 10.0 - side * 5.5, tip - dir * 10.0 + side * 5.5], theme::ACCENT, Stroke::new(1.0, shadow)));
        }
        painter.circle(at, 3.5, theme::ACCENT, Stroke::new(1.5, shadow));
        let label = painter.layout_no_wrap(tr("This face goes on the bed").to_string(), theme::mono(11.0), theme::TEXT);
        let chip = Rect::from_min_size(at + vec2(18.0, 14.0), label.size() + vec2(16.0, 10.0));
        painter.rect_filled(chip, CornerRadius::same(theme::RADIUS), Color32::from_black_alpha(215));
        painter.galley(chip.min + vec2(8.0, 5.0), label, theme::TEXT);
    }

    pub(super) fn draw_transform_gizmo(&self, ui: &Ui, viewport: Rect) {
        if self.tool != Tool::Move || !self.settings.show_overlays {
            return;
        }
        if self.lay_face_armed {
            self.draw_lay_preview(ui, viewport);
            return;
        }
        let Some(geo) = self.gizmo_geometry(viewport) else { return };
        let painter = ui.painter().with_clip_rect(viewport);
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let active = self.transform_drag.as_ref().map(|d| d.handle);
        let hot = active.or_else(|| pointer.and_then(|p| Self::hovered_handle(&geo, p)));
        let shadow = Color32::from_black_alpha(150);
        let up = self.settings.up_axis;
        let color = |k: usize, h: Handle| {
            let c = crate::axes::color(up.display(k).0);
            if hot == Some(h) { Color32::WHITE } else { c }
        };
        for k in 0..3 {
            let c = color(k, Handle::Ring(k));
            painter.add(egui::Shape::line(geo.rings[k].clone(), Stroke::new(4.0, shadow)));
            painter.add(egui::Shape::line(geo.rings[k].clone(), Stroke::new(if hot == Some(Handle::Ring(k)) { 3.0 } else { 2.0 }, c)));
        }
        for k in 0..3 {
            let Some(tip) = geo.arrows[k] else { continue };
            if tip.distance(geo.center) <= 12.0 {
                continue;
            }
            let c = color(k, Handle::Axis(k));
            painter.line_segment([geo.center, tip], Stroke::new(4.5, shadow));
            painter.line_segment([geo.center, tip], Stroke::new(if hot == Some(Handle::Axis(k)) { 3.0 } else { 2.2 }, c));
            let dir = (tip - geo.center).normalized();
            let side = vec2(-dir.y, dir.x);
            let head = vec![tip + dir * 9.0, tip - side * 5.5, tip + side * 5.5];
            painter.add(egui::Shape::convex_polygon(head, c, Stroke::new(1.0, shadow)));
        }
        let fill = if hot == Some(Handle::Center) { Color32::WHITE } else { Color32::from_white_alpha(200) };
        painter.circle(geo.center, CENTER_RADIUS * 0.55, fill, Stroke::new(1.5, shadow));
        painter.circle_stroke(geo.center, CENTER_RADIUS, Stroke::new(1.2, Color32::from_white_alpha(170)));

        // Live readout while dragging: what changed, in the units the file uses.
        if let Some(drag) = &self.transform_drag {
            let text = match drag.handle {
                Handle::Ring(_) => format!("{:.1}°", drag.angle.to_degrees().abs()),
                _ => {
                    let first = self.transform_targets().first().copied();
                    let moved = first.map_or(Vec3::ZERO, |i| (self.user_transforms[i] * drag.start[i].inverse()).w_axis.truncate());
                    let d = up.to_display(moved);
                    format!("X {}  Y {}  Z {}", self.fmt_model_len(d.x), self.fmt_model_len(d.y), self.fmt_model_len(d.z))
                }
            };
            let galley = painter.layout_no_wrap(text, theme::mono(11.5), theme::TEXT);
            let chip = Rect::from_min_size(geo.center + vec2(16.0, 16.0), galley.size() + vec2(16.0, 10.0));
            painter.rect_filled(chip, CornerRadius::same(theme::RADIUS), Color32::from_black_alpha(215));
            painter.galley(chip.min + vec2(8.0, 5.0), galley, theme::TEXT);
        }
    }

    /// Shortcuts that only apply with the Move tool.
    pub(super) fn transform_shortcuts(&mut self, ctx: &egui::Context) {
        if self.tool != Tool::Move || self.info.is_none() || !self.manufacturing() {
            return;
        }
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if pressed(Modifiers::ALT, Key::G) {
            self.reset_transforms();
        }
        if pressed(Modifiers::NONE, Key::L) {
            self.lay_face_armed = !self.lay_face_armed;
        }
        if self.lay_face_armed && pressed(Modifiers::NONE, Key::Escape) {
            self.lay_face_armed = false;
        }
        if !self.lay_face_armed {
            self.lay_hover = None;
            self.lay_hover_px = None;
        }
        if pressed(Modifiers::SHIFT, Key::L) {
            self.auto_orient(ctx);
        }
        if pressed(Modifiers::NONE, Key::B) {
            let targets = self.transform_targets();
            self.drop_to_bed(&targets, true);
        }
    }

    /// Move tool options, docked at the bottom of the viewport.
    pub(super) fn transform_bar(&mut self, ctx: &egui::Context, viewport: Rect) {
        if self.tool != Tool::Move || self.info.is_none() || !self.manufacturing() {
            return;
        }
        let targets = self.transform_targets();
        let bounds = self.targets_bounds(&targets);
        let mut action = None;
        egui::Area::new(Id::new("transform_bar"))
            .order(Order::Middle)
            .fixed_pos(pos2(viewport.center().x, viewport.bottom() - 12.0))
            .pivot(Align2::CENTER_BOTTOM)
            .show(ctx, |ui| {
                toolbar(ui, |ui| {
                    ui.add_space(6.0);
                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    icons::transform(ui.painter(), r, theme::TEXT_DIM);
                    // Size of what the gizmo moves, so a part can be checked against the bed.
                    let size = if bounds.is_valid() {
                        let s = self.settings.up_axis.to_display(bounds.max - bounds.min).abs();
                        format!("{} × {} × {}", self.fmt_model_len(s.x), self.fmt_model_len(s.y), self.fmt_model_len(s.z))
                    } else {
                        tr("Select an object").to_string()
                    };
                    let (r, _) = ui.allocate_exact_size(vec2(170.0, theme::TOOLBAR_HEIGHT), Sense::hover());
                    ui.painter().text(r.left_center(), Align2::LEFT_CENTER, size, theme::mono(11.5), theme::TEXT);
                    toolbar_separator(ui);
                    ui.add_enabled_ui(!targets.is_empty(), |ui| {
                        if text_button(ui, "Lay on face", self.lay_face_armed).on_hover_text(tr("Click a face to put it down on the bed (L)")).clicked() {
                            action = Some(BarAction::ArmLay);
                        }
                        if text_button(ui, "Auto orient", false).on_hover_text(tr("Lay the model on its largest flat side (Shift L)")).clicked() {
                            action = Some(BarAction::Auto);
                        }
                        if text_button(ui, "Drop to bed", false).on_hover_text(tr("Move down until it touches the bed (B)")).clicked() {
                            action = Some(BarAction::Drop);
                        }
                        if text_button(ui, "Reset", false).on_hover_text(tr("Back to the position in the file (Alt G)")).clicked() {
                            action = Some(BarAction::Reset);
                        }
                    });
                    toolbar_separator(ui);
                    if text_button(ui, "Export…", false).on_hover_text(tr("Save the moved model as STL or 3MF")).clicked() {
                        action = Some(BarAction::Export);
                    }
                    ui.add_space(4.0);
                });
            });
        match action {
            Some(BarAction::ArmLay) => self.lay_face_armed = !self.lay_face_armed,
            Some(BarAction::Auto) => self.auto_orient(ctx),
            Some(BarAction::Drop) => self.drop_to_bed(&targets, true),
            Some(BarAction::Reset) => self.reset_transforms(),
            Some(BarAction::Export) => self.export_model_dialog(ctx),
            None => {}
        }
    }

    pub(super) fn fmt_model_len(&self, v: f32) -> String {
        fmt_len(v)
    }

    // --- Export --------------------------------------------------------------------------------

    pub(super) fn export_model_dialog(&mut self, ctx: &egui::Context) {
        let Some(info) = &self.info else { return };
        if self.snap.is_empty() {
            self.show_toast(ctx, tr("Still analyzing the model, try again in a moment").to_string(), false);
            return;
        }
        let stem = info.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "model".into());
        let Some(path) = rfd::FileDialog::new()
            .set_title(tr("Export Model"))
            .set_file_name(format!("{stem}-placed.3mf"))
            .add_filter("3MF", &["3mf"])
            .add_filter("STL", &["stl"])
            .save_file()
        else {
            return;
        };
        let result = self.export_model_to(&path);
        match result {
            Ok(()) => self.show_toast(ctx, trf("Model saved: {name}", &[("name", &file_name(&path))]), false),
            Err(e) => self.show_toast(ctx, trf("Couldn't save {name}: {error}", &[("name", &file_name(&path)), ("error", &e)]), true),
        }
    }

    /// Writes the visible objects where the Move tool put them: 3MF or STL by extension.
    pub(super) fn export_model_to(&self, path: &Path) -> Result<(), String> {
        let info = self.info.as_ref().ok_or("no model")?;
        // Files that declare units were converted to meters on load: slicers expect millimeters.
        let scale = match info.units {
            crate::scene::Units::Undeclared => 1.0,
            _ => 1000.0,
        };
        let parts: Vec<ExportPart> = self
            .snap
            .iter()
            .enumerate()
            .filter(|(i, _)| self.visible.get(*i).copied().unwrap_or(true))
            .map(|(i, s)| ExportPart {
                name: info.objects.get(i).map(|o| o.name.clone()).unwrap_or_default(),
                positions: s.positions.iter().map(|&p| s.model.transform_point3(p) * scale).collect(),
                indices: s.indices.clone(),
            })
            .collect();
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("3mf")) { write_3mf(path, &parts) } else { write_stl(path, &parts) }
    }
}

#[derive(Clone, Copy)]
enum BarAction {
    ArmLay,
    Auto,
    Drop,
    Reset,
    Export,
}

struct ExportPart {
    name: String,
    positions: Vec<Vec3>,
    indices: Vec<u32>,
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Binary STL, every part in one solid.
fn write_stl(path: &Path, parts: &[ExportPart]) -> Result<(), String> {
    let triangles: usize = parts.iter().map(|p| p.indices.len() / 3).sum();
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut w = std::io::BufWriter::new(file);
    let mut header = [0u8; 80];
    let tag = b"PolyLoupe export";
    header[..tag.len()].copy_from_slice(tag);
    let io = |e: std::io::Error| e.to_string();
    w.write_all(&header).map_err(io)?;
    w.write_all(&(triangles as u32).to_le_bytes()).map_err(io)?;
    for part in parts {
        for t in part.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| part.positions.get(i as usize).copied().unwrap_or(Vec3::ZERO));
            let n = (b - a).cross(c - a).normalize_or_zero();
            for v in [n, a, b, c] {
                for x in v.to_array() {
                    w.write_all(&x.to_le_bytes()).map_err(io)?;
                }
            }
            w.write_all(&[0, 0]).map_err(io)?;
        }
    }
    w.flush().map_err(io)
}

/// 3MF with one object per part, in millimeters. Split vertices (STL-style) are welded back
/// so slicers see closed meshes.
fn write_3mf(path: &Path, parts: &[ExportPart]) -> Result<(), String> {
    use std::fmt::Write as _;
    let mut model = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n<resources>\n",
    );
    for (k, part) in parts.iter().enumerate() {
        let mut welded: HashMap<[u32; 3], u32> = HashMap::new();
        let mut verts: Vec<Vec3> = Vec::new();
        let remap: Vec<u32> = part
            .positions
            .iter()
            .map(|p| {
                *welded.entry(p.to_array().map(f32::to_bits)).or_insert_with(|| {
                    verts.push(*p);
                    verts.len() as u32 - 1
                })
            })
            .collect();
        let name = part.name.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;");
        let _ = write!(model, "<object id=\"{}\" type=\"model\" name=\"{name}\"><mesh><vertices>\n", k + 1);
        for v in &verts {
            let _ = writeln!(model, "<vertex x=\"{}\" y=\"{}\" z=\"{}\"/>", v.x, v.y, v.z);
        }
        model.push_str("</vertices><triangles>\n");
        for t in part.indices.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| remap.get(i as usize).copied().unwrap_or(0));
            if a != b && b != c && a != c {
                let _ = writeln!(model, "<triangle v1=\"{a}\" v2=\"{b}\" v3=\"{c}\"/>");
            }
        }
        model.push_str("</triangles></mesh></object>\n");
    }
    model.push_str("</resources>\n<build>\n");
    for k in 0..parts.len() {
        let _ = writeln!(model, "<item objectid=\"{}\"/>", k + 1);
    }
    model.push_str("</build>\n</model>\n");

    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let entries: [(&str, &str); 3] = [
        (
            "[Content_Types].xml",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/></Types>\n",
        ),
        (
            "_rels/.rels",
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Target=\"/3D/3dmodel.model\" Id=\"rel0\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/></Relationships>\n",
        ),
        ("3D/3dmodel.model", &model),
    ];
    for (name, body) in entries {
        zip.start_file(name, options).map_err(|e| e.to_string())?;
        zip.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_3mf_reads_back() {
        let part = ExportPart {
            name: "tri & <part>".into(),
            positions: vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::X, Vec3::Y, Vec3::new(1.0, 1.0, 0.0)],
            indices: vec![0, 1, 2, 3, 5, 4],
        };
        let path = std::env::temp_dir().join("polyloupe_export_test.3mf");
        write_3mf(&path, &[part]).unwrap();
        let scene = crate::loader::load(&path).expect("export should load");
        let _ = std::fs::remove_file(&path);
        assert_eq!(scene.triangle_count(), 2);
        // The two triangles share their diagonal once welded.
        assert_eq!(scene.source_vertex_count, 4);
    }
}
