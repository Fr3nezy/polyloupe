//! Turntable orbit camera with Blender navigation semantics (Z-up world).

use crate::i18n::{tr, trf};
use std::f32::consts::{FRAC_PI_2, PI, TAU};

use glam::camera::rh::proj::directx;
use glam::{Mat4, Vec3};

use crate::scene::Aabb;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewState {
    pub target: Vec3,
    /// Rotation around world Z. 0 = looking from +X (Right view).
    pub yaw: f32,
    /// Elevation. +PI/2 = looking from above (Top view).
    pub pitch: f32,
    pub distance: f32,
}

impl ViewState {
    fn lerp(a: &ViewState, b: &ViewState, t: f32) -> ViewState {
        ViewState {
            target: a.target.lerp(b.target, t),
            yaw: a.yaw + shortest_angle(a.yaw, b.yaw) * t,
            pitch: a.pitch + (b.pitch - a.pitch) * t,
            distance: a.distance * (b.distance / a.distance).powf(t),
        }
    }
}

fn shortest_angle(from: f32, to: f32) -> f32 {
    let d = (to - from).rem_euclid(TAU);
    if d > PI { d - TAU } else { d }
}

/// Named axis-aligned views, like Blender's numpad views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisView {
    Front,
    Back,
    Right,
    Left,
    Top,
    Bottom,
}

impl AxisView {
    pub fn label(self) -> &'static str {
        match self {
            AxisView::Front => "Front",
            AxisView::Back => "Back",
            AxisView::Right => "Right",
            AxisView::Left => "Left",
            AxisView::Top => "Top",
            AxisView::Bottom => "Bottom",
        }
    }

    fn angles(self) -> (f32, f32) {
        match self {
            AxisView::Front => (-FRAC_PI_2, 0.0),
            AxisView::Back => (FRAC_PI_2, 0.0),
            AxisView::Right => (0.0, 0.0),
            AxisView::Left => (PI, 0.0),
            AxisView::Top => (-FRAC_PI_2, FRAC_PI_2),
            AxisView::Bottom => (-FRAC_PI_2, -FRAC_PI_2),
        }
    }

    /// The view that looks at the scene from the given world axis direction.
    pub fn from_axis(axis: Vec3) -> AxisView {
        if axis.x > 0.5 {
            AxisView::Right
        } else if axis.x < -0.5 {
            AxisView::Left
        } else if axis.y > 0.5 {
            AxisView::Back
        } else if axis.y < -0.5 {
            AxisView::Front
        } else if axis.z > 0.5 {
            AxisView::Top
        } else {
            AxisView::Bottom
        }
    }

    pub fn opposite(self) -> AxisView {
        match self {
            AxisView::Front => AxisView::Back,
            AxisView::Back => AxisView::Front,
            AxisView::Right => AxisView::Left,
            AxisView::Left => AxisView::Right,
            AxisView::Top => AxisView::Bottom,
            AxisView::Bottom => AxisView::Top,
        }
    }
}

pub struct Camera {
    pub view: ViewState,
    pub fov_y: f32,
    pub ortho: bool,
    /// Blender's "Auto Perspective": axis views go orthographic, orbiting goes back.
    pub auto_perspective: bool,
    /// Set when the view is exactly aligned to an axis view.
    pub axis_view: Option<AxisView>,
    /// Scene size, used to keep clip planes sensible.
    pub scene_radius: f32,
    anim: Option<ViewAnim>,
}

struct ViewAnim {
    from: ViewState,
    to: ViewState,
    t: f32,
}

const ANIM_SECONDS: f32 = 0.18;

impl Default for Camera {
    fn default() -> Self {
        Self {
            view: ViewState {
                target: Vec3::ZERO,
                yaw: (-60f32).to_radians(),
                pitch: 25f32.to_radians(),
                distance: 8.0,
            },
            fov_y: 40f32.to_radians(),
            ortho: false,
            auto_perspective: true,
            axis_view: None,
            scene_radius: 1.0,
            anim: None,
        }
    }
}

impl Camera {
    /// Unit vector from target towards the eye.
    pub fn back(&self) -> Vec3 {
        let (sy, cy) = self.view.yaw.sin_cos();
        let (sp, cp) = self.view.pitch.sin_cos();
        Vec3::new(cp * cy, cp * sy, sp)
    }

    pub fn right(&self) -> Vec3 {
        let (sy, cy) = self.view.yaw.sin_cos();
        Vec3::new(-sy, cy, 0.0)
    }

    pub fn up(&self) -> Vec3 {
        self.back().cross(self.right())
    }

    pub fn eye(&self) -> Vec3 {
        self.view.target + self.back() * self.view.distance
    }

    pub fn view_matrix(&self) -> Mat4 {
        let world_from_cam = Mat4::from_cols(
            self.right().extend(0.0),
            self.up().extend(0.0),
            self.back().extend(0.0),
            self.eye().extend(1.0),
        );
        world_from_cam.inverse()
    }

    fn ortho_half_height(&self) -> f32 {
        self.view.distance * (self.fov_y * 0.5).tan()
    }

    /// Reverse-Z projection (depth 1 = near): keeps precision for huge and tiny scenes alike.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        if self.ortho {
            let h = self.ortho_half_height();
            let w = h * aspect;
            let depth = (self.scene_radius * 4.0).max(self.view.distance * 4.0);
            directx::orthographic(-w, w, -h, h, self.view.distance + depth, self.view.distance - depth)
        } else {
            let near = (self.view.distance * 0.002).max(1e-5);
            directx::perspective_infinite_reverse(self.fov_y, aspect, near)
        }
    }

    /// World units covered by one screen pixel at the target's depth.
    pub fn world_per_pixel(&self, viewport_height_px: f32) -> f32 {
        2.0 * self.ortho_half_height() / viewport_height_px.max(1.0)
    }

    /// Advances the view transition. Returns true while animating.
    pub fn update(&mut self, dt: f32) -> bool {
        let Some(anim) = &mut self.anim else {
            return false;
        };
        anim.t = (anim.t + dt / ANIM_SECONDS).min(1.0);
        let e = 1.0 - (1.0 - anim.t).powi(3); // ease-out cubic
        self.view = ViewState::lerp(&anim.from, &anim.to, e);
        if anim.t >= 1.0 {
            self.view = anim.to;
            self.anim = None;
        }
        true
    }

    fn animate_to(&mut self, to: ViewState) {
        self.anim = Some(ViewAnim {
            from: self.view,
            to,
            t: 0.0,
        });
    }

    fn stop_anim(&mut self) {
        if let Some(anim) = self.anim.take() {
            self.view = anim.to;
        }
    }

    pub fn orbit(&mut self, dx_px: f32, dy_px: f32) {
        self.stop_anim();
        const SPEED: f32 = 0.0075;
        self.view.yaw -= dx_px * SPEED;
        self.view.pitch += dy_px * SPEED;
        self.view.pitch = self.view.pitch.clamp(-FRAC_PI_2, FRAC_PI_2);
        if self.axis_view.take().is_some() && self.auto_perspective {
            self.ortho = false;
        }
    }

    /// First-person look (Unity/Unreal right-drag): turns the view around the eye.
    pub fn look(&mut self, dx_px: f32, dy_px: f32) {
        let eye = self.eye();
        self.orbit(dx_px, dy_px);
        self.view.target = eye - self.back() * self.view.distance;
    }

    /// Moves eye and target together. `forward`, `right`, `up` are view-relative, in units of
    /// the orbit distance so the speed suits any scene scale.
    pub fn fly(&mut self, forward: f32, right: f32, up: f32) {
        self.stop_anim();
        let step = self.view.distance.max(1e-3);
        self.view.target += (-self.back() * forward + self.right() * right + Vec3::Z * up) * step;
        if self.axis_view.take().is_some() && self.auto_perspective {
            self.ortho = false;
        }
    }

    /// Snaps to the axis view closest to the current direction (ZBrush's Shift while rotating).
    pub fn snap_to_axis(&mut self) {
        let b = self.back();
        let a = b.abs();
        let axis = if a.x >= a.y && a.x >= a.z {
            Vec3::X * b.x.signum()
        } else if a.y >= a.z {
            Vec3::Y * b.y.signum()
        } else {
            Vec3::Z * b.z.signum()
        };
        self.set_axis_view(AxisView::from_axis(axis));
    }

    pub fn pan(&mut self, dx_px: f32, dy_px: f32, viewport_height_px: f32) {
        self.stop_anim();
        let s = self.world_per_pixel(viewport_height_px);
        self.view.target += (-self.right() * dx_px + self.up() * dy_px) * s;
    }

    /// Positive steps zoom in.
    pub fn zoom(&mut self, steps: f32) {
        self.stop_anim();
        let min = (self.scene_radius * 1e-4).max(1e-4);
        let max = (self.scene_radius * 1e3).max(1e3);
        self.view.distance = (self.view.distance * 0.85f32.powf(steps)).clamp(min, max);
    }

    pub fn set_axis_view(&mut self, view: AxisView) {
        let (yaw, pitch) = view.angles();
        self.animate_to(ViewState {
            yaw,
            pitch,
            ..self.anim.as_ref().map_or(self.view, |a| a.to)
        });
        self.axis_view = Some(view);
        if self.auto_perspective {
            self.ortho = true;
        }
    }

    pub fn toggle_ortho(&mut self) {
        self.ortho = !self.ortho;
    }

    pub fn frame(&mut self, bounds: &Aabb, animate: bool) {
        if !bounds.is_valid() {
            return;
        }
        let radius = bounds.radius().max(1e-4);
        self.scene_radius = self.scene_radius.max(radius);
        let to = ViewState {
            target: bounds.center(),
            distance: radius / (self.fov_y * 0.5).sin() * 0.92,
            ..self.view
        };
        if animate {
            self.animate_to(to);
        } else {
            self.anim = None;
            self.view = to;
        }
    }

    /// Frames a freshly loaded scene from Blender's default 3/4 angle.
    pub fn reset(&mut self, bounds: &Aabb) {
        self.scene_radius = bounds.radius().max(1e-4);
        self.view.yaw = (-60f32).to_radians();
        self.view.pitch = 25f32.to_radians();
        self.axis_view = None;
        self.ortho = false;
        self.frame(bounds, false);
    }

    pub fn view_name(&self) -> String {
        let projection = tr(if self.ortho { "Orthographic" } else { "Perspective" });
        let view = tr(self.axis_view.map_or("User", AxisView::label));
        trf("{view} {projection}", &[("view", &view), ("projection", &projection)])
    }
}
