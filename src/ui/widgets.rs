//! Poly Loupe controls. Buttons are quiet (transparent, #1c1c1c on hover) and invert to white
//! on black when selected; groups sit on a #111 toolbar with a hairline border.

use eframe::egui::{
    self, Color32, CornerRadius, Frame, Margin, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use super::icons::IconFn;
use super::theme;
use crate::i18n::tr;

/// Position of a button inside a group. Buttons in a group are separate pills now (4px apart,
/// all corners rounded); kept so callers can still describe their groups.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    Single,
    First,
    Middle,
    Last,
}

impl Segment {
    pub fn of(index: usize, count: usize) -> Segment {
        match (index, count) {
            (_, 1) => Segment::Single,
            (0, _) => Segment::First,
            (i, n) if i == n - 1 => Segment::Last,
            _ => Segment::Middle,
        }
    }
}

fn radius() -> CornerRadius {
    CornerRadius::same(theme::RADIUS)
}

/// Fill and foreground for a quiet button.
fn colors(selected: bool, hovered: bool, pressed: bool) -> (Color32, Color32) {
    match (selected, hovered || pressed) {
        (true, false) => (theme::ACCENT, theme::ON_ACCENT),
        (true, true) => (theme::ACCENT_HOVER, theme::ON_ACCENT),
        (false, _) if pressed => (theme::WIDGET_PRESS, theme::TEXT),
        (false, true) => (theme::WIDGET_HOVER, theme::TEXT),
        (false, false) => (Color32::TRANSPARENT, theme::TEXT_DIM),
    }
}

fn focus_ring(ui: &Ui, rect: Rect, response: &Response) {
    if response.has_focus() {
        ui.painter().rect_stroke(rect.expand(2.0), radius(), Stroke::new(2.0, theme::TEXT), StrokeKind::Outside);
    }
}

/// A #111 toolbar with a hairline border around a row of buttons.
pub fn toolbar<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(theme::SURFACE)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(radius())
        .inner_margin(Margin::same(4))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal(add).inner
        })
        .inner
}

/// Thin vertical separator inside a toolbar.
pub fn toolbar_separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(9.0, 20.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, theme::BORDER));
}

pub fn icon_button(ui: &mut Ui, icon: IconFn, selected: bool, _segment: Segment, width: f32) -> Response {
    sized_icon_button(ui, icon, selected, Vec2::new(width.max(theme::TOOLBAR_HEIGHT), theme::TOOLBAR_HEIGHT))
}

pub fn sized_icon_button(ui: &mut Ui, icon: IconFn, selected: bool, size: Vec2) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let (fill, fg) = colors(selected, response.hovered(), response.is_pointer_button_down_on());
        let painter = ui.painter();
        painter.rect_filled(rect, radius(), fill);
        let side = (size.y * 0.6).min(20.0);
        icon(painter, Rect::from_center_size(rect.center(), Vec2::splat(side)), fg);
        focus_ring(ui, rect, &response);
    }
    response
}

/// Text button for toolbars ("Prospettiva"), 32 px tall.
pub fn text_button(ui: &mut Ui, text: &str, selected: bool) -> Response {
    let font = egui::FontId::proportional(13.0);
    let width = ui.painter().layout_no_wrap(tr(text).to_string(), font.clone(), theme::TEXT).size().x;
    let size = Vec2::new(width + 24.0, theme::TOOLBAR_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let (fill, fg) = colors(selected, response.hovered(), response.is_pointer_button_down_on());
        let fg = if selected || response.hovered() { fg } else { theme::TEXT };
        ui.painter().rect_filled(rect, radius(), fill);
        // Laid out with the final color: a galley's own color wins over the one passed to paint it.
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, tr(text), font, fg);
        focus_ring(ui, rect, &response);
    }
    response
}

/// White call-to-action button.
pub fn primary_button(ui: &mut Ui, text: &str, height: f32, hint: Option<&str>) -> Response {
    let font = theme::medium(if height >= 40.0 { 15.0 } else { 13.0 });
    let galley = ui.painter().layout_no_wrap(tr(text).to_string(), font, theme::ON_ACCENT);
    let hint = hint.map(|h| ui.painter().layout_job(theme::caps(h, 11.0, theme::TEXT_FAINT)));
    let extra = hint.as_ref().map_or(0.0, |h| h.size().x + 8.0);
    let size = Vec2::new(galley.size().x + extra + if height >= 40.0 { 40.0 } else { 24.0 }, height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if response.hovered() { theme::ACCENT_HOVER } else { theme::ACCENT };
        ui.painter().rect_filled(rect, radius(), fill);
        let text_w = galley.size().x + extra;
        let x = rect.center().x - text_w * 0.5;
        ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y * 0.5), galley.clone(), theme::ON_ACCENT);
        if let Some(h) = hint {
            let hx = x + galley.size().x + 8.0;
            ui.painter().galley(egui::pos2(hx, rect.center().y - h.size().y * 0.5 + 1.0), h, theme::TEXT_FAINT);
        }
        focus_ring(ui, rect, &response);
    }
    response
}

/// Tool rail button, 40x40.
pub fn tool_button(ui: &mut Ui, icon: IconFn, selected: bool) -> Response {
    sized_icon_button(ui, icon, selected, Vec2::splat(40.0))
}

fn segment_row<T: PartialEq + Copy>(
    ui: &mut Ui,
    options: &[(T, &str)],
    is_selected: impl Fn(T) -> bool,
) -> Option<T> {
    let mut clicked = None;
    let outer = ui.available_width();
    Frame::new()
        .fill(theme::BG_APP)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(radius())
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let gaps = 2.0 * (options.len().saturating_sub(1)) as f32;
            let width = ((outer - 6.0 - gaps) / options.len() as f32).floor();
            ui.horizontal(|ui| {
                for (opt, label) in options {
                    let selected = is_selected(*opt);
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(width, theme::CONTROL_HEIGHT - 4.0), Sense::click());
                    let (fill, fg) = colors(selected, response.hovered(), response.is_pointer_button_down_on());
                    let fg = if selected || response.hovered() { fg } else { theme::TEXT_DIM };
                    let painter = ui.painter();
                    painter.rect_filled(rect, CornerRadius::same(theme::RADIUS - 1), fill);
                    painter.text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        tr(label),
                        egui::FontId::proportional(12.5),
                        fg,
                    );
                    if response.clicked() {
                        clicked = Some(*opt);
                    }
                }
            });
        });
    clicked
}

/// Row of mutually exclusive text options; returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let current = *value;
    match segment_row(ui, options, |o| o == current) {
        Some(o) if o != current => {
            *value = o;
            true
        }
        _ => false,
    }
}


/// Section title: monospace, uppercase, tracked out, dim.
pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(theme::caps(tr(title), 11.0, theme::TEXT_DIM));
    ui.add_space(2.0);
}


/// Footer hint: white keys, dim action, e.g. "SHIFT+MMB SPOSTA".
pub fn hint(ui: &mut Ui, keys: &[&str], action: &str) {
    let keys: Vec<&str> = keys.iter().map(|k| tr(k)).collect();
    let format = |color| egui::TextFormat {
        font_id: theme::mono(11.0),
        color,
        extra_letter_spacing: 11.0 * 0.08,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(&keys.join("+").to_uppercase(), 0.0, format(theme::TEXT));
    job.append(&format!(" {}", tr(action).to_uppercase()), 0.0, format(theme::TEXT_DIM));
    ui.label(job);
    ui.add_space(8.0);
}


/// Key/value row inside a bordered table (Info panel).
pub fn table<R>(ui: &mut Ui, add: impl FnOnce(&mut TableRows) -> R) -> R {
    Frame::new()
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(radius())
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut rows = TableRows { ui, first: true };
            add(&mut rows)
        })
        .inner
}

pub struct TableRows<'a> {
    ui: &'a mut Ui,
    first: bool,
}

impl TableRows<'_> {
    pub fn row(&mut self, key: &str, value: &str) {
        let ui = &mut *self.ui;
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 38.0), Sense::hover());
        if !self.first {
            ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme::BORDER));
        }
        self.first = false;
        let p = ui.painter();
        let value = p.layout_no_wrap(value.to_string(), theme::medium(14.0), theme::TEXT);
        let value_left = rect.right() - 12.0 - value.size().x;
        p.galley(egui::pos2(value_left, rect.center().y - value.size().y * 0.5), value, theme::TEXT);
        // Long keys (file names) are clipped before the value.
        let clip = Rect::from_min_max(rect.min, egui::pos2(value_left - 10.0, rect.bottom()));
        p.with_clip_rect(clip).text(rect.left_center() + Vec2::new(12.0, 0.0), egui::Align2::LEFT_CENTER, tr(key), egui::FontId::proportional(14.0), theme::TEXT_DIM);
    }
}

/// Big number with a caption (Info panel stat cards).
pub fn stat_card(ui: &mut Ui, value: &str, caption: &str, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 58.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, radius(), theme::SURFACE);
    // Big counts ("4.030.464") shrink to fit the card instead of being cut off.
    let room = width - 24.0;
    let mut size = 22.0;
    let mut galley = p.layout_no_wrap(value.to_string(), theme::medium(size), theme::TEXT);
    if galley.size().x > room {
        size = (size * room / galley.size().x).floor().max(12.0);
        galley = p.layout_no_wrap(value.to_string(), theme::medium(size), theme::TEXT);
    }
    // Keep the baseline where the 22 px text sits.
    let y = rect.top() + 10.0 + (theme::medium(22.0).size - size) * 0.8;
    p.with_clip_rect(rect.shrink(1.0)).galley(egui::pos2(rect.left() + 12.0, y), galley, theme::TEXT);
    p.text(rect.left_bottom() + Vec2::new(12.0, -10.0), egui::Align2::LEFT_BOTTOM, tr(caption), egui::FontId::proportional(12.0), theme::TEXT_DIM);
}

/// Small bordered tag in monospace ("FBX", "4 !").
pub fn tag(ui: &mut Ui, text: &str, strong: bool) -> Response {
    let job = theme::caps(text, 10.0, if strong { theme::TEXT } else { theme::TEXT_DIM });
    let galley = ui.painter().layout_job(job);
    let size = galley.size() + Vec2::new(12.0, 4.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let stroke = Stroke::new(1.0, if strong { theme::BORDER_STRONG } else { theme::BORDER });
    ui.painter().rect_stroke(rect, CornerRadius::same(2), stroke, StrokeKind::Inside);
    ui.painter().galley(rect.center() - galley.size() * 0.5, galley, theme::TEXT);
    response
}

/// Dashed rectangle outline (drop zones, missing texture swatches).
pub fn dashed_rect(painter: &egui::Painter, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let (dash, gap) = (4.0, 3.0);
    let edges = [
        (rect.left_top(), rect.right_top()),
        (rect.right_top(), rect.right_bottom()),
        (rect.right_bottom(), rect.left_bottom()),
        (rect.left_bottom(), rect.left_top()),
    ];
    for (a, b) in edges {
        painter.extend(egui::Shape::dashed_line(&[a, b], stroke, dash, gap));
    }
}
