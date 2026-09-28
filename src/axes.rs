//! Which axis the interface calls "up". The scene is always Z-up inside (like Blender, and every
//! loader converts to it); with Y up (Maya, Unity, Houdini, ZBrush, Substance) only what the user
//! reads changes: axis names and colors, dimensions and measured deltas. The mapping is the one
//! exporters use between the two worlds: display = (x, z, -y).

use eframe::egui::Color32;
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::ui::theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UpAxis {
    /// Blender, 3ds Max, Unreal, most CAD.
    #[default]
    Z,
    /// Maya, Unity, Houdini, ZBrush, Substance.
    Y,
}

pub const NAMES: [&str; 3] = ["X", "Y", "Z"];

impl UpAxis {
    /// The internal axis (and sign) shown as display axis `d` (0 X, 1 Y, 2 Z).
    pub fn internal(self, d: usize) -> (usize, f32) {
        match self {
            UpAxis::Z => (d, 1.0),
            UpAxis::Y => [(0, 1.0), (2, 1.0), (1, -1.0)][d],
        }
    }

    /// The display axis (and sign) that internal axis `i` is shown as.
    pub fn display(self, i: usize) -> (usize, f32) {
        match self {
            UpAxis::Z => (i, 1.0),
            UpAxis::Y => [(0, 1.0), (2, -1.0), (1, 1.0)][i],
        }
    }

    /// An internal vector (or point) in display coordinates.
    pub fn to_display(self, v: Vec3) -> Vec3 {
        match self {
            UpAxis::Z => v,
            UpAxis::Y => Vec3::new(v.x, v.z, -v.y),
        }
    }
}

/// Color of display axis `d`: X red, Y green, Z blue, whatever the convention.
pub fn color(d: usize) -> Color32 {
    [theme::AXIS_X, theme::AXIS_Y, theme::AXIS_Z][d]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_internal_are_inverse() {
        for up in [UpAxis::Z, UpAxis::Y] {
            for d in 0..3 {
                let (i, s) = up.internal(d);
                assert_eq!(up.display(i), (d, s), "{up:?} {d}");
            }
        }
    }

    #[test]
    fn y_up_matches_maya() {
        // Blender's +Z (up) is Maya's +Y; Blender's -Y (toward the front camera) is Maya's +Z.
        let up = UpAxis::Y;
        assert_eq!(up.to_display(Vec3::Z), Vec3::Y);
        assert_eq!(up.to_display(Vec3::NEG_Y), Vec3::Z);
        let v = Vec3::new(1.0, 2.0, 3.0);
        let d = up.to_display(v);
        for k in 0..3 {
            let (i, s) = up.internal(k);
            assert_eq!(d[k], v[i] * s);
        }
    }
}
