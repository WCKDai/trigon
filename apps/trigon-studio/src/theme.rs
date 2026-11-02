//! Studio's workspace identity and spacing; drawing colours and model materials are independent.

use eframe::egui;

pub const INK: egui::Color32 = egui::Color32::from_rgb(236, 238, 240);
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(169, 176, 183);
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(255, 112, 53);
pub const PANEL: egui::Color32 = egui::Color32::from_rgb(41, 44, 47);
pub const CHROME: egui::Color32 = egui::Color32::from_rgb(32, 34, 37);
pub const LINE: egui::Color32 = egui::Color32::from_rgb(66, 71, 77);
pub const PAPER: egui::Color32 = egui::Color32::from_rgb(245, 245, 242);
pub const DRAWING_INK: egui::Color32 = egui::Color32::from_rgb(37, 43, 43);

/// Portable UI typography distributed under SIL OFL 1.1 in Studio assets.
pub fn apply(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, data, family) in [
        ("Inter", include_bytes!("../assets/fonts/inter/Inter-400.ttf").as_slice(), egui::FontFamily::Proportional),
        ("IBM Plex Mono", include_bytes!("../assets/fonts/plex-mono/IBM-Plex-Mono-400.ttf").as_slice(), egui::FontFamily::Monospace),
        ("Inter Medium", include_bytes!("../assets/fonts/inter/Inter-500.ttf").as_slice(), egui::FontFamily::Name("Inter Medium".into())),
        ("Inter Semibold", include_bytes!("../assets/fonts/inter/Inter-600.ttf").as_slice(), egui::FontFamily::Name("Inter Semibold".into())),
    ] {
        fonts.font_data.insert(name.into(), std::sync::Arc::new(egui::FontData::from_static(data)));
        fonts.families.entry(family).or_default().insert(0, name.into());
    }
    let fallback = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    for name in ["Inter Medium", "Inter Semibold"] {
        fonts.families.entry(egui::FontFamily::Name(name.into())).or_default().extend(fallback.clone());
    }
    ctx.set_fonts(fonts);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = CHROME;
    style.visuals.faint_bg_color = egui::Color32::from_rgb(52, 56, 61);
    style.visuals.override_text_color = Some(INK);
    style.visuals.hyperlink_color = ACCENT;
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(73, 51, 41);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, LINE);
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, MUTED);
    style.visuals.widgets.inactive.weak_bg_fill = CHROME;
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(52, 56, 61);
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, LINE);
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, INK);
    style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(52, 56, 61);
    style.visuals.widgets.hovered.bg_fill = LINE;
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    style.visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(73, 51, 41);
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    style.visuals.window_corner_radius = egui::CornerRadius::same(5);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(4);
    for widget in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.noninteractive,
    ] {
        widget.corner_radius = egui::CornerRadius::same(3);
    }
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.interact_size.y = 28.0;
    style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
    style.text_styles.insert(egui::TextStyle::Button, egui::FontId::new(13.0, egui::FontFamily::Name("Inter Medium".into())));
    style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(11.0));
    style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::new(18.0, egui::FontFamily::Name("Inter Semibold".into())));
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_theme(egui::ThemePreference::Dark);
}

/// Owner-supplied gradient identity, preserving the original SVG proportions.
pub fn mark(ui: &mut egui::Ui, width: f32) {
    crate::icons::show(ui, crate::icons::Icon::Logo, width);
}

/// Compact wordmark in the Studio UI's semibold family.
pub fn wordmark(ui: &mut egui::Ui) {
    mark(ui, 24.0);
    ui.label(egui::RichText::new("TRIGON").font(egui::FontId::new(18.0, egui::FontFamily::Name("Inter Semibold".into()))).color(INK));
}

pub fn workspace_tab(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let response = ui.add(egui::Button::selectable(selected, label).frame(false).min_size(egui::vec2(92.0, 32.0)));
    if selected {
        let rect = response.rect;
        ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], egui::Stroke::new(3.0, ACCENT));
    }
    response
}

pub fn primary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(egui::RichText::new(label).strong().color(CHROME)).fill(ACCENT))
}

pub fn eyebrow(ui: &mut egui::Ui, label: &str) {
    ui.add_space(5.0);
    ui.label(egui::RichText::new(label).size(10.0).strong().color(MUTED));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_portable_typography_fit_compact_chrome() {
        let ctx = egui::Context::default();
        apply(&ctx);
        let mut output = ctx.run_ui(
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 120.0))), ..Default::default() },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        wordmark(ui);
                        ui.label("STUDIO");
                    });
                    let galley = ui.painter().layout_no_wrap("Køkken · ÆØÅ 0123456789".into(), egui::FontId::proportional(13.0), INK);
                    assert!(galley.size().x < 220.0 && galley.size().y < 25.0);
                    assert!(ui.min_rect().right() <= 420.0);
                });
            },
        );
        output.textures_delta.clear();
        assert!(!output.shapes.is_empty());
        let style = ctx.style_of(egui::Theme::Dark);
        assert_eq!(style.visuals.widgets.inactive.corner_radius, egui::CornerRadius::same(3));
        let linear = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        let luminance = |c: egui::Color32| 0.2126 * linear(c.r()) + 0.7152 * linear(c.g()) + 0.0722 * linear(c.b());
        assert!((luminance(INK) + 0.05) / (luminance(PANEL) + 0.05) > 5.0);
        assert!((luminance(PAPER) + 0.05) / (luminance(DRAWING_INK) + 0.05) > 5.0);
    }
}
