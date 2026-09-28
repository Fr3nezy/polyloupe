//! Blender-style timeline: transport buttons, clip picker, scrub bar with keyframes, speed, loop.

use crate::i18n::{tr, trf};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
    pos2, vec2,
};

use super::widgets::{self, Segment};
use super::{icons, theme};

pub enum TimelineAction {
    TogglePlay,
    JumpStart,
    JumpEnd,
    Step(i32),
    SetFrame(i32),
    SetClip(usize),
    SetSpeed(f32),
    ToggleLoop,
}

pub struct TimelineView<'a> {
    pub clips: Vec<&'a str>,
    pub clip: Option<usize>,
    pub frame: i32,
    pub last_frame: i32,
    pub playing: bool,
    pub speed: f32,
    pub looping: bool,
    pub fps: f32,
    /// Keyframe positions of the current clip, in frames.
    pub keys: &'a [i32],
}

const SPEEDS: [f32; 6] = [0.25, 0.5, 1.0, 1.5, 2.0, 4.0];

pub fn show(ui: &mut Ui, v: &TimelineView) -> Vec<TimelineAction> {
    let mut actions = Vec::new();
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let buttons: [(icons::IconFn, &str, TimelineAction); 5] = [
            (icons::jump_start, tr("Jump to start (Shift ←)"), TimelineAction::JumpStart),
            (icons::step_back, tr("Previous frame (←)"), TimelineAction::Step(-1)),
            (
                if v.playing { icons::pause } else { icons::play },
                if v.playing { tr("Pause (Space)") } else { tr("Play (Space)") },
                TimelineAction::TogglePlay,
            ),
            (icons::step_forward, tr("Next frame (→)"), TimelineAction::Step(1)),
            (icons::jump_end, tr("Jump to end (Shift →)"), TimelineAction::JumpEnd),
        ];
        for (i, (icon, tip, action)) in buttons.into_iter().enumerate() {
            let is_play = i == 2;
            let r = widgets::icon_button(ui, icon, is_play && v.playing, Segment::of(i, 5), 28.0).on_hover_text(tip);
            if r.clicked() {
                actions.push(action);
            }
        }
        ui.add_space(10.0);
        ui.spacing_mut().item_spacing.x = 8.0;

        if v.clips.len() > 1 {
            let current = v.clip.and_then(|c| v.clips.get(c)).copied().unwrap_or("—");
            egui::ComboBox::from_id_salt("clip")
                .selected_text(current)
                .width(150.0)
                .show_ui(ui, |ui| {
                    for (i, name) in v.clips.iter().enumerate() {
                        if ui.selectable_label(v.clip == Some(i), *name).clicked() {
                            actions.push(TimelineAction::SetClip(i));
                        }
                    }
                })
                .response
                .on_hover_text(tr("Animation clip"));
        } else if let Some(name) = v.clips.first() {
            ui.label(egui::RichText::new(*name).color(theme::TEXT_DIM));
        }

        let right = 250.0;
        let width = (ui.available_width() - right).max(120.0);
        if let Some(frame) = scrub_bar(ui, v, width) {
            actions.push(TimelineAction::SetFrame(frame));
        }

        let counter = format!("{} / {}", v.frame, v.last_frame);
        ui.label(egui::RichText::new(counter).monospace().color(theme::TEXT))
            .on_hover_text(trf(
                "{seconds} s at {fps} fps",
                &[("seconds", &format!("{:.2}", v.frame as f32 / v.fps.max(1.0))), ("fps", &v.fps)],
            ));
        egui::ComboBox::from_id_salt("speed")
            .selected_text(format!("{}×", trim(v.speed)))
            .width(56.0)
            .show_ui(ui, |ui| {
                for s in SPEEDS {
                    if ui.selectable_label((v.speed - s).abs() < 1e-3, format!("{}×", trim(s))).clicked() {
                        actions.push(TimelineAction::SetSpeed(s));
                    }
                }
            })
            .response
            .on_hover_text(tr("Playback speed"));
        ui.spacing_mut().item_spacing.x = 0.0;
        let r = widgets::icon_button(ui, icons::repeat, v.looping, Segment::Single, 28.0).on_hover_text(tr("Loop"));
        if r.clicked() {
            actions.push(TimelineAction::ToggleLoop);
        }
    });
    actions
}

fn trim(v: f32) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Draws the frame ruler and returns a frame when the user clicks or drags on it.
fn scrub_bar(ui: &mut Ui, v: &TimelineView, width: f32) -> Option<i32> {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 30.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect.expand(2.0));
    painter.rect(
        rect,
        CornerRadius::same(theme::RADIUS),
        Color32::from_rgb(0x22, 0x22, 0x22),
        Stroke::new(1.0, theme::BORDER),
        StrokeKind::Inside,
    );
    let inner = rect.shrink2(vec2(10.0, 0.0));
    let last = v.last_frame.max(1);
    let x_of = |f: i32| inner.left() + inner.width() * f as f32 / last as f32;

    // Ticks: pick a step so labels stay at least ~45 px apart.
    let px_per_frame = inner.width() / last as f32;
    let step = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000]
        .into_iter()
        .find(|s| *s as f32 * px_per_frame >= 45.0)
        .unwrap_or(10000);
    let mut f = 0;
    while f <= last {
        let x = x_of(f);
        painter.line_segment([pos2(x, rect.bottom() - 8.0), pos2(x, rect.bottom() - 2.0)], Stroke::new(1.0, Color32::from_gray(80)));
        painter.text(pos2(x, rect.top() + 3.0), Align2::CENTER_TOP, f.to_string(), FontId::proportional(10.0), theme::TEXT_DIM);
        f += step;
    }

    // Keyframes as small diamonds along the bottom.
    for &k in v.keys {
        let c = pos2(x_of(k), rect.bottom() - 6.0);
        painter.add(Shape::convex_polygon(
            vec![c + vec2(0.0, -3.5), c + vec2(3.5, 0.0), c + vec2(0.0, 3.5), c + vec2(-3.5, 0.0)],
            Color32::from_rgb(0xd9, 0xc9, 0x7a),
            Stroke::new(1.0, Color32::from_black_alpha(160)),
        ));
    }

    // Playhead with its frame number, Blender style.
    let x = x_of(v.frame.clamp(0, last));
    painter.line_segment([pos2(x, rect.top() + 2.0), pos2(x, rect.bottom() - 2.0)], Stroke::new(2.0, theme::ACCENT));
    let label = v.frame.to_string();
    let galley = painter.layout_no_wrap(label, FontId::proportional(10.5), theme::ON_ACCENT);
    let badge = Rect::from_center_size(pos2(x, rect.top() + 9.0), galley.size() + vec2(8.0, 2.0));
    painter.rect_filled(badge, CornerRadius::same(3), theme::ACCENT);
    painter.galley(badge.center() - galley.size() * 0.5, galley, theme::ON_ACCENT);

    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let pos = response.interact_pointer_pos()?;
    if response.clicked() || response.dragged() {
        let t = ((pos.x - inner.left()) / inner.width()).clamp(0.0, 1.0);
        return Some((t * last as f32).round() as i32);
    }
    None
}
