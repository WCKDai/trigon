//! Studio's workspace identity and spacing; drawing colours and model materials are independent.

use eframe::egui;

pub const INK: egui::Color32 = egui::Color32::from_rgb(237, 239, 233);
pub const MUTED: egui::Color32 = egui::Color32::from_rgb(156, 166, 167);
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(220, 112, 63);
pub const PANEL: egui::Color32 = egui::Color32::from_rgb(27, 31, 32);
pub const CHROME: egui::Color32 = egui::Color32::from_rgb(20, 24, 25);
pub const LINE: egui::Color32 = egui::Color32::from_rgb(58, 64, 65);
pub const PAPER: egui::Color32 = egui::Color32::from_rgb(245, 241, 231);
pub const DRAWING_INK: egui::Color32 = egui::Color32::from_rgb(37, 43, 43);

/// Reuse the portable Liberation family already distributed under SIL OFL 1.1 in engine/assets.
pub fn apply(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, data, family) in [
        (
            "Liberation Sans",
            include_bytes!("../../../crates/engine/assets/liberation-sans/LiberationSans-Regular.ttf").as_slice(),
            egui::FontFamily::Proportional,
        ),
        (
            "Liberation Mono",
            include_bytes!("../../../crates/engine/assets/liberation/LiberationMono-Regular.ttf").as_slice(),
            egui::FontFamily::Monospace,
        ),
    ] {
        fonts.font_data.insert(name.into(), std::sync::Arc::new(egui::FontData::from_static(data)));
        fonts.families.entry(family).or_default().insert(0, name.into());
    }
    ctx.set_fonts(fonts);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = CHROME;
    style.visuals.faint_bg_color = egui::Color32::from_rgb(33, 38, 39);
    style.visuals.override_text_color = Some(INK);
    style.visuals.hyperlink_color = ACCENT;
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(72, 43, 31);
    style.visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, LINE);
    style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, MUTED);
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(32, 37, 38);
    style.visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(48, 54, 55);
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, LINE);
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, INK);
    style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(50, 57, 58);
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(59, 65, 66);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    style.visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(72, 43, 31);
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
    style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
    style.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(11.0));
    style.text_styles.insert(egui::TextStyle::Heading, egui::FontId::proportional(18.0));
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.set_theme(egui::ThemePreference::Dark);
}

/// Original Trigon identity: nine separated triangular tiles, drawn as crisp native paths.
pub fn mark(ui: &mut egui::Ui, width: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, width * 0.84), egui::Sense::hover());
    let tiles = [
        [[3.0, 0.0], [2.0, 1.0], [4.0, 1.0]],
        [[2.0, 1.0], [1.0, 2.0], [3.0, 2.0]],
        [[2.0, 1.0], [4.0, 1.0], [3.0, 2.0]],
        [[4.0, 1.0], [3.0, 2.0], [5.0, 2.0]],
        [[1.0, 2.0], [0.0, 3.0], [2.0, 3.0]],
        [[1.0, 2.0], [3.0, 2.0], [2.0, 3.0]],
        [[3.0, 2.0], [2.0, 3.0], [4.0, 3.0]],
        [[3.0, 2.0], [5.0, 2.0], [4.0, 3.0]],
        [[5.0, 2.0], [4.0, 3.0], [6.0, 3.0]],
    ];
    for triangle in tiles {
        let centre = [(triangle[0][0] + triangle[1][0] + triangle[2][0]) / 3.0, (triangle[0][1] + triangle[1][1] + triangle[2][1]) / 3.0];
        let points = triangle
            .iter()
            .map(|p| {
                let x = centre[0] + (p[0] - centre[0]) * 0.90;
                let y = centre[1] + (p[1] - centre[1]) * 0.90;
                egui::pos2(rect.left() + x / 6.0 * rect.width(), rect.top() + y / 3.0 * rect.height())
            })
            .collect();
        ui.painter().add(egui::Shape::convex_polygon(points, ACCENT, egui::Stroke::NONE));
    }
}

/// Original vector mark paired with a spaced text wordmark; no reference brand assets.
pub fn wordmark(ui: &mut egui::Ui) {
    mark(ui, 28.0);
    ui.label(egui::RichText::new("T R I G O N").size(18.0).strong().color(INK));
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
