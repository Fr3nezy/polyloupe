//! Viewport navigation presets: mouse mappings borrowed from the tools artists already know.
//!
//! Everything else (selection, shading keys, the Z pie) stays Blender's; only how the mouse
//! moves the camera changes, plus F to frame the selection where that tool uses it.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Navigation {
    Blender,
    /// Also Cinema 4D, Substance Painter, Marmoset Toolbag, Houdini.
    Maya,
    ZBrush,
    Max,
    /// Unity and Unreal: right-drag to look, WASD to fly.
    GameEngine,
    /// SolidWorks-style.
    Cad,
}

/// What a drag does this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Orbit,
    Pan,
    Zoom,
    Look,
}

/// Mouse buttons held in a drag, plus modifiers.
#[derive(Clone, Copy, Default)]
pub struct DragInput {
    pub left: bool,
    pub middle: bool,
    pub right: bool,
    pub alt: bool,
    pub shift: bool,
    pub ctrl: bool,
}

impl Navigation {
    pub const ALL: [Navigation; 6] = [
        Navigation::Blender,
        Navigation::Maya,
        Navigation::ZBrush,
        Navigation::Max,
        Navigation::GameEngine,
        Navigation::Cad,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Navigation::Blender => "Blender",
            Navigation::Maya => "Maya · Cinema 4D · Substance",
            Navigation::ZBrush => "ZBrush",
            Navigation::Max => "3ds Max",
            Navigation::GameEngine => "Unity · Unreal",
            Navigation::Cad => "CAD (SolidWorks)",
        }
    }

    /// Maps a drag to a camera gesture. `None` leaves the drag alone (LMB stays selection).
    pub fn gesture(self, d: DragInput) -> Option<Gesture> {
        use Gesture::*;
        match self {
            // Alt+LMB is Blender's "Emulate 3 Button Mouse", essential on laptops.
            Navigation::Blender => (d.middle || (d.left && d.alt)).then_some(if d.shift {
                Pan
            } else if d.ctrl {
                Zoom
            } else {
                Orbit
            }),
            Navigation::Maya => match (d.alt, d.left, d.middle, d.right) {
                (true, true, _, _) => Some(Orbit),
                (true, _, true, _) => Some(Pan),
                (true, _, _, true) => Some(Zoom),
                (false, _, true, _) => Some(Pan),
                _ => None,
            },
            Navigation::ZBrush => {
                (d.left || d.right).then_some(if d.alt { Pan } else if d.ctrl { Zoom } else { Orbit })
            }
            Navigation::Max => d.middle.then_some(if d.alt && d.ctrl {
                Zoom
            } else if d.alt {
                Orbit
            } else {
                Pan
            }),
            Navigation::GameEngine => {
                if d.alt && d.left {
                    Some(Orbit)
                } else if d.alt && d.right {
                    Some(Zoom)
                } else if d.right {
                    Some(Look)
                } else {
                    d.middle.then_some(Pan)
                }
            }
            Navigation::Cad => d.middle.then_some(if d.ctrl {
                Pan
            } else if d.shift {
                Zoom
            } else {
                Orbit
            }),
        }
    }

    /// F frames the selection in every preset but Blender (which uses `.`).
    pub fn f_frames(self) -> bool {
        self != Navigation::Blender
    }

    /// Status bar hints: (keys, action).
    pub fn hints(self) -> &'static [(&'static [&'static str], &'static str)] {
        match self {
            Navigation::Blender => &[(&["MMB"], "Orbit"), (&["Shift", "MMB"], "Pan"), (&["Wheel"], "Zoom")],
            Navigation::Maya => &[(&["Alt", "LMB"], "Orbit"), (&["Alt", "MMB"], "Pan"), (&["Alt", "RMB"], "Zoom")],
            Navigation::ZBrush => &[(&["RMB"], "Orbit"), (&["Alt", "RMB"], "Pan"), (&["Ctrl", "RMB"], "Zoom")],
            Navigation::Max => &[(&["Alt", "MMB"], "Orbit"), (&["MMB"], "Pan"), (&["Wheel"], "Zoom")],
            Navigation::GameEngine => &[(&["RMB"], "Look around"), (&["RMB", "WASD"], "Fly"), (&["Alt", "LMB"], "Orbit")],
            Navigation::Cad => &[(&["MMB"], "Orbit"), (&["Ctrl", "MMB"], "Pan"), (&["Wheel"], "Zoom")],
        }
    }

    /// Help menu rows: (keys, action).
    pub fn help(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Navigation::Blender => &[("MMB  /  Alt LMB", "Orbit"), ("Shift MMB", "Pan"), ("Wheel  /  Ctrl MMB", "Zoom")],
            Navigation::Maya => &[
                ("Alt LMB", "Orbit"),
                ("Alt MMB  /  MMB", "Pan"),
                ("Alt RMB  /  Wheel", "Zoom"),
                ("F", "Frame selected"),
            ],
            Navigation::ZBrush => &[
                ("LMB  /  RMB drag", "Orbit"),
                ("Shift while rotating", "Snap to the nearest view"),
                ("Alt LMB  /  Alt RMB", "Pan"),
                ("Ctrl RMB  /  Wheel", "Zoom"),
                ("F", "Frame selected"),
            ],
            Navigation::Max => &[
                ("Alt MMB", "Orbit"),
                ("MMB", "Pan"),
                ("Ctrl Alt MMB  /  Wheel", "Zoom"),
                ("F", "Frame selected"),
            ],
            Navigation::GameEngine => &[
                ("RMB drag", "Look around"),
                ("RMB + W A S D  /  Q E", "Fly (Shift faster)"),
                ("Alt LMB", "Orbit"),
                ("MMB", "Pan"),
                ("Alt RMB  /  Wheel", "Zoom"),
                ("F", "Frame selected"),
            ],
            Navigation::Cad => &[
                ("MMB", "Orbit"),
                ("Ctrl MMB", "Pan"),
                ("Shift MMB  /  Wheel", "Zoom"),
                ("F", "Frame selected"),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_left_drag_selects_except_in_zbrush() {
        let lmb = DragInput { left: true, ..Default::default() };
        for nav in Navigation::ALL {
            assert_eq!(nav.gesture(lmb).is_some(), nav == Navigation::ZBrush, "{nav:?}");
        }
    }

    #[test]
    fn presets_match_their_tools() {
        let alt = |b: DragInput| DragInput { alt: true, ..b };
        let l = DragInput { left: true, ..Default::default() };
        let m = DragInput { middle: true, ..Default::default() };
        let r = DragInput { right: true, ..Default::default() };
        assert_eq!(Navigation::Blender.gesture(m), Some(Gesture::Orbit));
        assert_eq!(Navigation::Blender.gesture(alt(l)), Some(Gesture::Orbit));
        assert_eq!(Navigation::Maya.gesture(alt(l)), Some(Gesture::Orbit));
        assert_eq!(Navigation::Maya.gesture(alt(r)), Some(Gesture::Zoom));
        assert_eq!(Navigation::ZBrush.gesture(r), Some(Gesture::Orbit));
        assert_eq!(Navigation::ZBrush.gesture(alt(r)), Some(Gesture::Pan));
        assert_eq!(Navigation::Max.gesture(m), Some(Gesture::Pan));
        assert_eq!(Navigation::Max.gesture(alt(m)), Some(Gesture::Orbit));
        assert_eq!(Navigation::GameEngine.gesture(r), Some(Gesture::Look));
        assert_eq!(Navigation::Cad.gesture(DragInput { ctrl: true, ..m }), Some(Gesture::Pan));
    }
}
