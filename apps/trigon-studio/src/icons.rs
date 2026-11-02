//! Unmodified Figma assets, rasterized once per display density with the drawing renderer.

use eframe::egui;
use trigon_engine::resvg::{self, tiny_skia, usvg};

#[derive(Clone, Copy, Debug)]
pub enum Icon {
    Logo,
    Undo,
    Redo,
    Save,
    Export,
    Search,
    Select,
    Move,
    Copy,
    Align,
    Levels,
    Component,
    Wall,
    Door,
    Window,
    Stair,
    Roof,
    Slab,
    Railing,
    Section,
    Map,
    Box,
    Bookmark,
}

impl Icon {
    fn source(self) -> &'static [u8] {
        match self {
            Self::Logo => include_bytes!("../assets/figma/Group.svg"),
            Self::Undo => include_bytes!("../assets/figma/LucideUndo2.svg"),
            Self::Redo => include_bytes!("../assets/figma/LucideRedo2.svg"),
            Self::Save => include_bytes!("../assets/figma/LucideSave.svg"),
            Self::Export => include_bytes!("../assets/figma/LucideUpload.svg"),
            Self::Search => include_bytes!("../assets/figma/LucideSearch.svg"),
            Self::Select => include_bytes!("../assets/figma/LucideMousePointer2.svg"),
            Self::Move => include_bytes!("../assets/figma/LucideMove.svg"),
            Self::Copy => include_bytes!("../assets/figma/LucideCopy.svg"),
            Self::Align => include_bytes!("../assets/figma/LucideScanLine.svg"),
            Self::Levels => include_bytes!("../assets/figma/TrigonBimLevel.svg"),
            Self::Component => include_bytes!("../assets/figma/LucideComponent.svg"),
            Self::Wall => include_bytes!("../assets/figma/TrigonBimWall.svg"),
            Self::Door => include_bytes!("../assets/figma/LucideDoorOpen.svg"),
            Self::Window => include_bytes!("../assets/figma/TrigonBimWindow.svg"),
            Self::Stair => include_bytes!("../assets/figma/TrigonBimStair.svg"),
            Self::Roof => include_bytes!("../assets/figma/TrigonBimRoof.svg"),
            Self::Slab => include_bytes!("../assets/figma/TrigonBimSlab.svg"),
            Self::Railing => include_bytes!("../assets/figma/TrigonBimRailing.svg"),
            Self::Section => include_bytes!("../assets/figma/TrigonBimSection.svg"),
            Self::Map => include_bytes!("../assets/figma/LucideMap.svg"),
            Self::Box => include_bytes!("../assets/figma/LucideBox.svg"),
            Self::Bookmark => include_bytes!("../assets/figma/LucideBookmark.svg"),
        }
    }

    fn texture(self, ui: &egui::Ui) -> Option<egui::TextureHandle> {
        let density = ui.ctx().pixels_per_point().clamp(1.0, 4.0).ceil() as u32;
        let id = egui::Id::new(("figma-icon", format!("{self:?}"), density));
        if let Some(texture) = ui.ctx().data_mut(|data| data.get_temp::<egui::TextureHandle>(id)) {
            return Some(texture);
        }
        let image = raster(self.source(), density as f32)?;
        let texture = ui.ctx().load_texture(format!("{self:?}-{density}"), image, egui::TextureOptions::LINEAR);
        ui.ctx().data_mut(|data| data.insert_temp(id, texture.clone()));
        Some(texture)
    }
}

fn raster(source: &[u8], density: f32) -> Option<egui::ColorImage> {
    let tree = usvg::Tree::from_data(source, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let mut pixels = tiny_skia::Pixmap::new((size.width() * density).ceil() as u32, (size.height() * density).ceil() as u32)?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(density, density), &mut pixels.as_mut());
    Some(egui::ColorImage::from_rgba_premultiplied([pixels.width() as usize, pixels.height() as usize], pixels.data()))
}

pub fn show(ui: &mut egui::Ui, icon: Icon, width: f32) {
    if let Some(texture) = icon.texture(ui) {
        let size = texture.size_vec2();
        ui.add(egui::Image::from_texture((texture.id(), size)).fit_to_exact_size(egui::vec2(width, width * size.y / size.x)));
    }
}

pub fn button(ui: &mut egui::Ui, icon: Icon, label: &str, selected: bool) -> egui::Response {
    if let Some(texture) = icon.texture(ui) {
        let image = egui::Image::from_texture((texture.id(), egui::vec2(18.0, 18.0)));
        ui.add(egui::Button::image_and_text(image, label).selected(selected))
    } else {
        ui.button(label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supplied_assets_render_transparently_at_retina_density() {
        for icon in [
            Icon::Logo,
            Icon::Undo,
            Icon::Redo,
            Icon::Save,
            Icon::Export,
            Icon::Search,
            Icon::Select,
            Icon::Move,
            Icon::Copy,
            Icon::Align,
            Icon::Levels,
            Icon::Component,
            Icon::Wall,
            Icon::Door,
            Icon::Window,
            Icon::Stair,
            Icon::Roof,
            Icon::Slab,
            Icon::Railing,
            Icon::Section,
            Icon::Map,
            Icon::Box,
            Icon::Bookmark,
        ] {
            let image = raster(icon.source(), 2.0).unwrap();
            assert!(image.pixels.iter().any(|p| p.a() > 0), "empty {icon:?}");
            assert!(image.pixels.iter().any(|p| p.a() == 0), "opaque backdrop {icon:?}");
            if !matches!(icon, Icon::Logo) {
                assert_eq!(image.size, [48, 48]);
            }
        }
        let logo = raster(Icon::Logo.source(), 2.0).unwrap();
        assert_eq!(logo.size, [120, 104]);
        assert!(
            logo.pixels.iter().filter(|p| p.a() == 255).map(|p| p.to_array()).collect::<std::collections::BTreeSet<_>>().len() > 20,
            "retain the supplied gradient rather than a flat redrawn mark"
        );
    }
}
