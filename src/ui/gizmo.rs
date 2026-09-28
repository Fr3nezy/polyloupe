//! Navigation gizmo (top-right of the viewport), same behavior as Blender's:
//! click an axis to look along it, click it again to flip, drag to orbit.
//! Sits on a dark disc with a hairline border, like the rest of the Poly Loupe chrome.

use crate::i18n::{tr, trf};
use eframe::egui::{
    Align2, Color32, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2, vec2,
};
use glam::Vec3;

use super::theme;
use crate::axes::UpAxis;
use crate::camera::{AxisView, Camera};

pub enum GizmoAction {
    Orbit(Vec2),
    AxisView(AxisView),
}

const AXIS_LEN: f32 = 38.0;
const BUBBLE: f32 = 9.0;

struct Axis {
    dir: Vec3,
    color: Color32,
    label: &'static str,
    positive: bool,
}

/// The six world directions: internal axis index and sign.
const DIRS: [(Vec3, usize, f32); 6] = [
    (Vec3::X, 0, 1.0),
    (Vec3::Y, 1, 1.0),
    (Vec3::Z, 2, 1.0),
    (Vec3::NEG_X, 0, -1.0),
    (Vec3::NEG_Y, 1, -1.0),
    (Vec3::NEG_Z, 2, -1.0),
];

/// Bubbles named and colored after the display convention: with Y up, world +Z is "Y"
/// (green) and world -Y is "Z" (blue).
fn axes(up: UpAxis) -> Vec<Axis> {
    DIRS.iter()
        .map(|&(dir, i, sign)| {
            let (d, s) = up.display(i);
            let positive = sign * s > 0.0;
            let label = if positive { crate::axes::NAMES[d] } else { ["-X", "-Y", "-Z"][d] };
            Axis { dir, color: crate::axes::color(d), label, positive }
        })
        .collect()
}

/// Diameter of the gizmo disc, used to place it.
pub const WIDTH: f32 = (AXIS_LEN + BUBBLE + 6.0) * 2.0;
const DISC: f32 = WIDTH * 0.5;

pub fn show(ui: &mut Ui, center: Pos2, camera: &Camera, up: UpAxis) -> Vec<GizmoAction> {
    let axes = axes(up);
    let mut actions = Vec::new();
    let (right, up, back) = (camera.right(), camera.up(), camera.back());

    let area = Rect::from_center_size(center, Vec2::splat((AXIS_LEN + BUBBLE + 4.0) * 2.0));
    let response = ui.interact(area, Id::new("nav_gizmo"), Sense::click_and_drag());
    let pointer = response.hover_pos();
    let painter = ui.painter();

    // Project axes and sort back to front.
    let mut projected: Vec<(f32, Pos2, &Axis)> = axes
        .iter()
        .map(|a| {
            let offset = vec2(a.dir.dot(right), -a.dir.dot(up)) * AXIS_LEN;
            (a.dir.dot(back), center + offset, a)
        })
        .collect();
    projected.sort_by(|a, b| a.0.total_cmp(&b.0));

    // Front-most bubble under the pointer wins.
    let hovered_axis = pointer.and_then(|p| {
        projected
            .iter()
            .rev()
            .find(|(_, pos, _)| pos.distance(p) <= BUBBLE + 1.0)
            .map(|(_, _, a)| a.dir)
    });

    let disc_fill = if response.hovered() || response.dragged() {
        Color32::from_rgba_unmultiplied(0x1c, 0x1c, 0x1c, 200)
    } else {
        Color32::from_black_alpha(170)
    };
    painter.circle(center, DISC, disc_fill, Stroke::new(1.0, theme::BORDER));

    let font = theme::bold(10.0);
    for (depth, pos, axis) in &projected {
        let hovered = hovered_axis == Some(axis.dir);
        // Axes pointing away fade a little, like Blender.
        let fade = 0.65 + 0.35 * (depth * 0.5 + 0.5);
        let color = axis.color.gamma_multiply(fade);
        if axis.positive {
            painter.line_segment([center, *pos], Stroke::new(2.0, color));
            painter.circle_filled(*pos, BUBBLE, color);
            painter.text(*pos, Align2::CENTER_CENTER, axis.label, font.clone(), theme::ON_ACCENT);
        } else {
            painter.circle_filled(*pos, BUBBLE, axis.color.gamma_multiply(0.28));
            painter.circle_stroke(*pos, BUBBLE - 0.5, Stroke::new(1.0, color));
        }
        if hovered {
            painter.circle_stroke(*pos, BUBBLE + 1.5, Stroke::new(1.5, Color32::WHITE));
            if !axis.positive {
                painter.text(*pos, Align2::CENTER_CENTER, axis.label, font.clone(), Color32::WHITE);
            }
        }
    }

    if response.clicked() {
        if let Some(dir) = hovered_axis {
            let view = AxisView::from_axis(dir);
            // Clicking the axis we're already looking along flips to the opposite side.
            let view = if camera.axis_view == Some(view) { view.opposite() } else { view };
            actions.push(GizmoAction::AxisView(view));
        }
    }
    if response.dragged() {
        actions.push(GizmoAction::Orbit(response.drag_delta()));
    }
    if let Some(dir) = hovered_axis {
        let view = AxisView::from_axis(dir);
        response.on_hover_text(trf("{view} view", &[("view", &tr(view.label()))]));
    } else {
        response.on_hover_text(tr("Drag to orbit · click an axis to align the view"));
    }

    actions
}
