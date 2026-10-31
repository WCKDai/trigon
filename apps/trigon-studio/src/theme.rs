//! Studio's shared workspace colours and spacing; model materials remain in trigon-model.

use eframe::egui;

pub const INK: egui::Color32 = egui::Color32::from_rgb(32, 43, 52);
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(111, 125, 137);
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(30, 122, 116);
pub const PANEL: egui::Color32 = egui::Color32::from_rgb(249, 250, 252);
pub const LINE: egui::Color32 = egui::Color32::from_rgb(220, 226, 232);

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.style_of(egui::Theme::Light)).clone();
    style.visuals = egui::Visuals::light();
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = egui::Color32::WHITE;
    style.visuals.extreme_bg_color = egui::Color32::from_rgb(239, 243, 246);
    style.visuals.override_text_color = Some(INK);
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(218, 237, 233);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, LINE);
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::WHITE;
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, LINE);
    style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(231, 242, 239);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    style.spacing.item_spacing = egui::vec2(8.0, 7.0);
    style.spacing.button_padding = egui::vec2(10.0, 6.0);
    style.spacing.interact_size.y = 28.0;
    style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
    style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
    style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(11.0));
    style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(17.0));
    ctx.set_style_of(egui::Theme::Light, style);
    ctx.set_theme(egui::ThemePreference::Light);
}

pub fn eyebrow(ui: &mut egui::Ui, label: &str) {
    ui.add_space(5.0);
    ui.label(egui::RichText::new(label).size(10.0).strong().color(MUTED));
}
