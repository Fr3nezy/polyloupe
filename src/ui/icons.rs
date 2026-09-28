//! Vector icons drawn with the egui painter: crisp at any DPI, no icon font or image assets.
//! Shapes follow Blender's viewport header icons so they read instantly.

use std::f32::consts::TAU;

use eframe::egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2, vec2};

pub type IconFn = fn(&Painter, Rect, Color32);

fn ellipse(center: Pos2, radii: Vec2, segments: usize) -> Vec<Pos2> {
    (0..segments)
        .map(|i| {
            let a = i as f32 / segments as f32 * TAU;
            center + vec2(a.cos() * radii.x, a.sin() * radii.y)
        })
        .collect()
}

fn icon_radius(rect: Rect) -> f32 {
    rect.width().min(rect.height()) * 0.36
}

pub fn wireframe(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let r = icon_radius(rect);
    let stroke = Stroke::new(1.2, color);
    p.circle_stroke(c, r, stroke);
    p.add(Shape::closed_line(ellipse(c, vec2(r * 0.42, r), 28), stroke));
    p.add(Shape::closed_line(ellipse(c, vec2(r, r * 0.42), 28), stroke));
}

pub fn solid(p: &Painter, rect: Rect, color: Color32) {
    p.circle_filled(rect.center(), icon_radius(rect), color);
}

pub fn rendered(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let r = icon_radius(rect);
    p.circle_filled(c, r, color.gamma_multiply(0.45));
    p.circle_filled(c + vec2(-r * 0.22, -r * 0.22), r * 0.72, color.gamma_multiply(0.8));
    p.circle_filled(c + vec2(-r * 0.38, -r * 0.38), r * 0.3, color);
}

pub fn xray(p: &Painter, rect: Rect, color: Color32) {
    let s = icon_radius(rect) * 1.7;
    let back = Rect::from_center_size(rect.center() + vec2(s * 0.14, -s * 0.14), Vec2::splat(s * 0.72));
    let front = Rect::from_center_size(rect.center() + vec2(-s * 0.14, s * 0.14), Vec2::splat(s * 0.72));
    p.rect_stroke(back, 1.5, Stroke::new(1.2, color), eframe::egui::StrokeKind::Middle);
    p.rect_filled(front, 1.5, color.gamma_multiply(0.45));
    p.rect_stroke(front, 1.5, Stroke::new(1.2, color), eframe::egui::StrokeKind::Middle);
}

pub fn overlays(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let r = icon_radius(rect) * 0.72;
    let stroke = Stroke::new(1.2, color);
    p.circle_stroke(c + vec2(-r * 0.5, 0.0), r, stroke);
    p.circle_filled(c + vec2(r * 0.5, 0.0), r, color.gamma_multiply(0.45));
    p.circle_stroke(c + vec2(r * 0.5, 0.0), r, stroke);
}

pub fn chevron_down(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.16;
    p.add(Shape::line(
        vec![c + vec2(-s, -s * 0.5), c + vec2(0.0, s * 0.5), c + vec2(s, -s * 0.5)],
        Stroke::new(1.3, color),
    ));
}

/// Three horizontal sliders: "settings of this group".
pub fn sliders(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let w = icon_radius(rect);
    let stroke = Stroke::new(1.2, color);
    for (dy, knob) in [(-0.6, 0.35), (0.0, -0.45), (0.6, 0.1)] {
        let y = c.y + dy * w;
        p.line_segment([Pos2::new(c.x - w, y), Pos2::new(c.x + w, y)], stroke);
        p.circle_filled(Pos2::new(c.x + knob * w, y), 2.3, color);
    }
}




/// Outliner visibility toggle.
pub fn eye(p: &Painter, rect: Rect, color: Color32, open: bool) {
    let c = rect.center();
    let w = rect.width().min(rect.height()) * 0.42;
    let stroke = Stroke::new(1.2, color);
    let lid = |sign: f32| -> Vec<Pos2> {
        (0..=12)
            .map(|i| {
                let t = i as f32 / 12.0 * 2.0 - 1.0;
                c + vec2(t * w, sign * (1.0 - t * t) * w * 0.55)
            })
            .collect()
    };
    if open {
        p.add(Shape::line(lid(-1.0), stroke));
        p.add(Shape::line(lid(1.0), stroke));
        p.circle_filled(c, w * 0.3, color);
    } else {
        p.add(Shape::line(lid(1.0), stroke));
        for t in [-0.5f32, 0.0, 0.5] {
            let base = c + vec2(t * w, (1.0 - t * t) * w * 0.55);
            p.line_segment([base, base + vec2(t * 2.0, 2.5)], Stroke::new(1.0, color));
        }
    }
}



fn triangle(p: &Painter, center: Pos2, size: f32, dir: f32, color: Color32) {
    p.add(Shape::convex_polygon(
        vec![
            center + vec2(size * 0.6 * dir, 0.0),
            center + vec2(-size * 0.5 * dir, -size * 0.6),
            center + vec2(-size * 0.5 * dir, size * 0.6),
        ],
        color,
        Stroke::NONE,
    ));
}

pub fn play(p: &Painter, rect: Rect, color: Color32) {
    triangle(p, rect.center() + vec2(1.0, 0.0), icon_radius(rect) * 1.2, 1.0, color);
}

pub fn pause(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let h = icon_radius(rect) * 0.8;
    for dx in [-2.5, 2.5] {
        p.rect_filled(Rect::from_center_size(c + vec2(dx, 0.0), vec2(2.6, h * 2.0)), 0.5, color);
    }
}

pub fn step_back(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let s = icon_radius(rect);
    triangle(p, c + vec2(1.5, 0.0), s, -1.0, color);
    p.rect_filled(Rect::from_center_size(c + vec2(-s * 0.75, 0.0), vec2(2.0, s * 1.4)), 0.0, color);
}

pub fn step_forward(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let s = icon_radius(rect);
    triangle(p, c + vec2(-1.5, 0.0), s, 1.0, color);
    p.rect_filled(Rect::from_center_size(c + vec2(s * 0.75, 0.0), vec2(2.0, s * 1.4)), 0.0, color);
}

pub fn jump_start(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let s = icon_radius(rect) * 0.8;
    triangle(p, c + vec2(-1.0, 0.0), s, -1.0, color);
    triangle(p, c + vec2(s * 0.9, 0.0), s, -1.0, color);
    p.rect_filled(Rect::from_center_size(c + vec2(-s * 1.2, 0.0), vec2(2.0, s * 1.4)), 0.0, color);
}

pub fn jump_end(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let s = icon_radius(rect) * 0.8;
    triangle(p, c + vec2(1.0, 0.0), s, 1.0, color);
    triangle(p, c + vec2(-s * 0.9, 0.0), s, 1.0, color);
    p.rect_filled(Rect::from_center_size(c + vec2(s * 1.2, 0.0), vec2(2.0, s * 1.4)), 0.0, color);
}

pub fn repeat(p: &Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let r = icon_radius(rect) * 0.85;
    let stroke = Stroke::new(1.4, color);
    let arc: Vec<Pos2> = (0..=20)
        .map(|i| {
            let a = 0.35 + i as f32 / 20.0 * (TAU - 1.2);
            c + vec2(a.cos() * r, a.sin() * r)
        })
        .collect();
    let end = *arc.last().expect("arc has points");
    p.add(Shape::line(arc, stroke));
    let dir = vec2(1.0, 0.4).normalized();
    triangle(p, end + dir * 1.5, 5.0, 1.0, color);
}

// --- Poly Loupe line icons, ported from the UI prototype (SVG viewBoxes of 12..18 units) ----

/// Maps a point of an `n` x `n` icon box onto `rect` (square, centered).
fn grid(rect: Rect, n: f32) -> impl Fn(f32, f32) -> Pos2 {
    let side = rect.width().min(rect.height());
    let origin = rect.center() - Vec2::splat(side * 0.5);
    let k = side / n;
    move |x, y| origin + vec2(x * k, y * k)
}

fn polyline(p: &Painter, pts: Vec<Pos2>, closed: bool, stroke: Stroke) {
    p.add(if closed { Shape::closed_line(pts, stroke) } else { Shape::line(pts, stroke) });
}

fn stroke_for(rect: Rect, n: f32, width: f32, color: Color32) -> Stroke {
    Stroke::new((width * rect.width().min(rect.height()) / n).max(1.0), color)
}

/// Selection tool: a cursor arrow.
pub fn cursor(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 18.0);
    polyline(p, vec![g(4.0, 2.5), g(14.0, 8.5), g(9.5, 9.7), g(7.2, 14.0)], true, stroke_for(rect, 18.0, 1.5, color));
}

/// Orbit tool: a point circled by an orbit.
pub fn orbit(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 18.0);
    let s = stroke_for(rect, 18.0, 1.5, color);
    let k = rect.width().min(rect.height()) / 18.0;
    p.circle_stroke(g(9.0, 9.0), 2.0 * k, s);
    p.add(Shape::closed_line(ellipse(g(9.0, 9.0), vec2(7.0 * k, 3.2 * k), 32), s));
}

/// Move tool: four arrows.
pub fn move_arrows(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 18.0);
    let s = stroke_for(rect, 18.0, 1.5, color);
    p.line_segment([g(9.0, 2.0), g(9.0, 16.0)], s);
    p.line_segment([g(2.0, 9.0), g(16.0, 9.0)], s);
    for (tip, a, b) in [
        ((9.0, 2.0), (7.0, 4.0), (11.0, 4.0)),
        ((9.0, 16.0), (7.0, 14.0), (11.0, 14.0)),
        ((2.0, 9.0), (4.0, 7.0), (4.0, 11.0)),
        ((16.0, 9.0), (14.0, 7.0), (14.0, 11.0)),
    ] {
        polyline(p, vec![g(a.0, a.1), g(tip.0, tip.1), g(b.0, b.1)], false, s);
    }
}

/// Zoom tool: magnifier with a plus.
/// Diagonal ruler with tick marks.
pub fn ruler(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 18.0);
    let s = stroke_for(rect, 18.0, 1.5, color);
    p.add(Shape::closed_line(vec![g(2.5, 12.0), g(12.0, 2.5), g(15.5, 6.0), g(6.0, 15.5)], s));
    for (x, y, len) in [(5.0, 9.5, 2.0), (7.5, 7.0, 3.0), (10.0, 4.5, 2.0)] {
        p.line_segment([g(x, y), g(x + len * 0.7, y + len * 0.7)], s);
    }
}

pub fn magnifier(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 18.0);
    let s = stroke_for(rect, 18.0, 1.5, color);
    let k = rect.width().min(rect.height()) / 18.0;
    p.circle_stroke(g(8.0, 8.0), 5.0 * k, s);
    p.line_segment([g(12.0, 12.0), g(16.0, 16.0)], s);
    p.line_segment([g(6.0, 8.0), g(10.0, 8.0)], s);
    p.line_segment([g(8.0, 6.0), g(8.0, 10.0)], s);
}

/// Frame the model: corner brackets around a square.
pub fn frame(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 16.0);
    let s = stroke_for(rect, 16.0, 1.4, color);
    for pts in [
        [(1.5, 5.0), (1.5, 1.5), (5.0, 1.5)],
        [(11.0, 1.5), (14.5, 1.5), (14.5, 5.0)],
        [(14.5, 11.0), (14.5, 14.5), (11.0, 14.5)],
        [(5.0, 14.5), (1.5, 14.5), (1.5, 11.0)],
    ] {
        polyline(p, pts.iter().map(|&(x, y)| g(x, y)).collect(), false, s);
    }
    polyline(p, vec![g(5.0, 5.0), g(11.0, 5.0), g(11.0, 11.0), g(5.0, 11.0)], true, s);
}

/// Side panel toggle: a window split on the right.
pub fn panel(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 16.0);
    let s = stroke_for(rect, 16.0, 1.4, color);
    polyline(p, vec![g(1.5, 2.5), g(14.5, 2.5), g(14.5, 13.5), g(1.5, 13.5)], true, s);
    p.line_segment([g(10.0, 2.5), g(10.0, 13.5)], s);
}

/// Outlined isometric cube (outliner rows, recent files).
pub fn cube_line(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 14.0);
    let s = stroke_for(rect, 14.0, 1.2, color);
    polyline(p, vec![g(7.0, 1.5), g(12.0, 4.3), g(12.0, 9.7), g(7.0, 12.5), g(2.0, 9.7), g(2.0, 4.3)], true, s);
    polyline(p, vec![g(2.0, 4.3), g(7.0, 7.1), g(12.0, 4.3)], false, s);
    p.line_segment([g(7.0, 7.1), g(7.0, 12.5)], s);
}

/// Warning triangle.
pub fn warning(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 16.0);
    let s = stroke_for(rect, 16.0, 1.4, color);
    polyline(p, vec![g(8.0, 1.8), g(14.5, 13.8), g(1.5, 13.8)], true, s);
    p.line_segment([g(8.0, 6.5), g(8.0, 10.0)], s);
    p.line_segment([g(8.0, 11.8), g(8.0, 12.2)], s);
}

/// Tree disclosure chevron.
pub fn chevron_small(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 14.0);
    polyline(p, vec![g(3.0, 5.0), g(7.0, 9.0), g(11.0, 5.0)], false, stroke_for(rect, 14.0, 1.3, color));
}

pub fn close(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 12.0);
    let s = stroke_for(rect, 12.0, 1.3, color);
    p.line_segment([g(2.0, 2.0), g(10.0, 10.0)], s);
    p.line_segment([g(10.0, 2.0), g(2.0, 10.0)], s);
}

pub fn window_minimize(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 12.0);
    p.line_segment([g(1.0, 6.0), g(11.0, 6.0)], Stroke::new(1.0, color));
}

pub fn window_maximize(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 12.0);
    polyline(p, vec![g(1.5, 1.5), g(10.5, 1.5), g(10.5, 10.5), g(1.5, 10.5)], true, Stroke::new(1.0, color));
}

pub fn window_restore(p: &Painter, rect: Rect, color: Color32) {
    let g = grid(rect, 12.0);
    let s = Stroke::new(1.0, color);
    polyline(p, vec![g(1.5, 3.5), g(8.5, 3.5), g(8.5, 10.5), g(1.5, 10.5)], true, s);
    polyline(p, vec![g(3.5, 3.5), g(3.5, 1.5), g(10.5, 1.5), g(10.5, 8.5), g(8.5, 8.5)], false, s);
}
