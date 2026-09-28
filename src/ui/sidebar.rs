//! Inspector actions shared by the Scene and Materials tabs (drawn in `app::shell`).

use crate::settings::TexturePass;

pub enum SidebarAction {
    Select { index: usize, extend: bool },
    ToggleVisible(usize),
    Frame(usize),
    #[allow(dead_code)]
    ShowPass(TexturePass),
}

/// The channel a texture map row stands for ("Roughness · G" -> Roughness).
pub fn pass_for(label: &str) -> Option<TexturePass> {
    let base = label.split(" · ").next().unwrap_or(label);
    Some(match base {
        "Base Color" => TexturePass::BaseColor,
        "Roughness" => TexturePass::Roughness,
        "Metallic" => TexturePass::Metallic,
        "Normal" => TexturePass::Normal,
        "Occlusion" => TexturePass::Occlusion,
        "Emission" => TexturePass::Emission,
        _ => return None,
    })
}
