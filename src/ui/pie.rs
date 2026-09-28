//! Blender's `Z` shading pie menu.
//!
//! Tap Z to open it and click a slice, or hold Z, flick towards a slice and release.
//! Layout matches Blender: Wireframe left, Solid right, Rendered top, X-ray bottom.

use crate::i18n::tr;
use std::f32::consts::FRAC_PI_4;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Id, Key, LayerId, Order, PointerButton, Pos2,
    Rect, Sense, Stroke, StrokeKind, Vec2, vec2,
};

use super::{icons, theme};
use crate::settings::ShadingMode;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PieChoice {
    Shading(ShadingMode),
    ToggleXray,
}

pub struct PieMenu {
    center: Option<Pos2>,
    opened_at: f64,
}

const RADIUS: f32 = 92.0;
const DEAD_ZONE: f32 = 18.0;
const HOLD_SECONDS: f64 = 0.25;

struct Slice {
    dir: Vec2,
    choice: PieChoice,
    label: &'static str,
    icon: icons::IconFn,
}

const SLICES: [Slice; 4] = [
    Slice { dir: Vec2::new(-1.0, 0.0), choice: PieChoice::Shading(ShadingMode::Wireframe), label: "Wireframe", icon: icons::wireframe },
    Slice { dir: Vec2::new(1.0, 0.0), choice: PieChoice::Shading(ShadingMode::Solid), label: "Solid", icon: icons::solid },
    Slice { dir: Vec2::new(0.0, -1.0), choice: PieChoice::Shading(ShadingMode::Rendered), label: "Rendered", icon: icons::rendered },
    Slice { dir: Vec2::new(0.0, 1.0), choice: PieChoice::ToggleXray, label: "Toggle X-Ray", icon: icons::xray },
];

impl PieMenu {
    pub fn new() -> Self {
        Self { center: None, opened_at: 0.0 }
    }

    pub fn is_open(&self) -> bool {
        self.center.is_some()
    }

    pub fn open(&mut self, at: Pos2, time: f64) {
        self.center = Some(at);
        self.opened_at = time;
    }

    /// Draws the pie and returns the picked choice, if any.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        current: ShadingMode,
        xray: bool,
    ) -> Option<PieChoice> {
        let center = self.center?;
        let (pointer, time, released_z, clicked, cancel) = ctx.input(|i| {
            (
                i.pointer.hover_pos(),
                i.time,
                i.key_released(Key::Z),
                i.pointer.button_clicked(PointerButton::Primary),
                i.key_pressed(Key::Escape) || i.pointer.button_clicked(PointerButton::Secondary),
            )
        });

        let active = pointer.and_then(|p| {
            let d = p - center;
            if d.length() < DEAD_ZONE {
                return None;
            }
            let d = d.normalized();
            SLICES.iter().position(|s| s.dir.dot(d) > FRAC_PI_4.cos())
        });

        let layer = LayerId::new(Order::Foreground, Id::new("shading_pie"));
        let painter = ctx.layer_painter(layer);
        // Swallow clicks so they don't reach the viewport under the pie.
        egui::Area::new(Id::new("shading_pie_blocker"))
            .order(Order::Foreground)
            .fixed_pos(center - Vec2::splat(RADIUS + 90.0))
            .show(ctx, |ui| {
                ui.allocate_exact_size(Vec2::splat((RADIUS + 90.0) * 2.0), Sense::click());
            });

        // Center ring with a direction indicator.
        painter.circle_filled(center, DEAD_ZONE, Color32::from_black_alpha(120));
        painter.circle_stroke(center, DEAD_ZONE, Stroke::new(2.0, Color32::from_white_alpha(60)));
        if let (Some(p), Some(_)) = (pointer, active) {
            let d = (p - center).normalized();
            let a = center + d * (DEAD_ZONE - 1.0);
            painter.line_segment([a - d.rot90() * 5.0, a + d.rot90() * 5.0], Stroke::new(3.0, theme::ACCENT));
        }

        let font = FontId::proportional(13.0);
        for (i, slice) in SLICES.iter().enumerate() {
            let galley = painter.layout_no_wrap(tr(slice.label).to_string(), font.clone(), theme::TEXT);
            let size = vec2(galley.size().x + 44.0, 28.0);
            let anchor = center + slice.dir * RADIUS;
            let rect = Rect::from_center_size(anchor, size);
            let selected = match slice.choice {
                PieChoice::Shading(m) => m == current,
                PieChoice::ToggleXray => xray,
            };
            let hot = active == Some(i);
            let fill = if hot { theme::ACCENT } else { theme::PANEL };
            let fg = if hot { theme::ON_ACCENT } else { theme::TEXT };
            painter.rect(rect, CornerRadius::same(14), fill, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
            let icon_rect = Rect::from_center_size(rect.left_center() + vec2(18.0, 0.0), Vec2::splat(20.0));
            (slice.icon)(&painter, icon_rect, if selected && !hot { theme::ACCENT } else { fg });
            painter.text(
                rect.left_center() + vec2(32.0, 0.0),
                Align2::LEFT_CENTER,
                tr(slice.label),
                font.clone(),
                fg,
            );
        }

        let held_long_enough = time - self.opened_at > HOLD_SECONDS;
        let pick = if clicked || (released_z && held_long_enough) {
            self.center = None;
            active.map(|i| SLICES[i].choice)
        } else {
            None
        };
        if cancel {
            self.center = None;
        }
        pick
    }
}
