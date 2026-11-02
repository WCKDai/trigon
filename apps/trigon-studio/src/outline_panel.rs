//! Exact exterior source lines for setting out plan guides. No fitted or rounded
//! coordinate is substituted for the stored building outline.

use eframe::egui;
use trigon_model::Document;

use crate::theme;

pub struct Guide {
    pub level: usize,
    pub line: [[f64; 2]; 2],
}

#[derive(Default)]
pub struct OutlinePanel {
    building: Option<String>,
    level: Option<usize>,
    edge: usize,
}

impl OutlinePanel {
    pub fn start(&mut self, level: Option<usize>) {
        self.level = level;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, doc: &Document, preferred_level: Option<usize>) -> Option<Guide> {
        theme::eyebrow(ui, "PRECISION / BUILDING OUTLINE");
        ui.heading("Set out from the source");
        ui.weak("Exact site-local exterior lines. Use a saved guide or a represented wall face when aligning interiors.");
        if self.building.as_ref().is_some_and(|id| !doc.buildings.iter().any(|b| &b.id == id)) {
            self.building = None;
        }
        if self.building.is_none() && doc.buildings.len() == 1 {
            self.building = Some(doc.buildings[0].id.clone());
            self.level = preferred_level;
        }
        let width = ui.available_width();
        let name = self.building.as_ref().and_then(|id| doc.buildings.iter().find(|b| &b.id == id)).map_or("Choose building", |b| b.name.as_str());
        egui::ComboBox::from_id_salt("outline-building").width(width).selected_text(name).show_ui(ui, |ui| {
            for b in &doc.buildings {
                if ui.selectable_value(&mut self.building, Some(b.id.clone()), &b.name).changed() {
                    self.level = None;
                    self.edge = 0;
                }
            }
        });
        let id = self.building.as_ref()?;
        let mut buildings = doc.buildings.iter().filter(|b| &b.id == id);
        let b = buildings.next()?;
        if buildings.next().is_some() {
            ui.colored_label(theme::ACCENT, "The building ID is ambiguous.");
            return None;
        }
        if self.level.is_some_and(|level| level >= b.storeys.len()) {
            self.level = None;
        }
        let level_name = self.level.and_then(|level| b.storeys.get(level)).map_or("Choose floor", |storey| storey.name.as_str());
        egui::ComboBox::from_id_salt("outline-level").width(width).selected_text(level_name).show_ui(ui, |ui| {
            for (index, storey) in b.storeys.iter().enumerate() {
                if ui.selectable_value(&mut self.level, Some(index), &storey.name).changed() {
                    self.edge = 0;
                }
            }
        });
        let level = self.level?;
        ui.label(format!("Floor elevation: {:.6} mm", b.storeys.get(level)?.elevation));
        let (ring, source_edges) = b.storey_ring(level);
        if ring.len() < 3 {
            ui.weak("This storey has no valid exterior ring.");
            return None;
        }
        self.edge = self.edge.min(ring.len() - 1);
        egui::ComboBox::from_id_salt("outline-edge").width(width).selected_text(format!("Edge {} / {}", self.edge + 1, ring.len())).show_ui(
            ui,
            |ui| {
                for index in 0..ring.len() {
                    let source = source_edges.get(index).copied().unwrap_or(index);
                    let facade = b.facades.iter().find(|f| f.edge == source).map_or("source", |f| f.id.as_str());
                    ui.selectable_value(&mut self.edge, index, format!("{} · {facade}", index + 1));
                }
            },
        );
        let line = [ring[self.edge], ring[(self.edge + 1) % ring.len()]];
        let delta = [line[1][0] - line[0][0], line[1][1] - line[0][1]];
        ui.add_space(8.0);
        ui.label(format!("Length: {:.6} mm", delta[0].hypot(delta[1])));
        ui.label(format!("Angle: {:.6}° from site east", delta[1].atan2(delta[0]).to_degrees()));
        for (label, point) in ["START", "END"].into_iter().zip(line) {
            theme::eyebrow(ui, label);
            ui.add(egui::Label::new(format!("East X: {:.6} mm", point[0])).wrap());
            ui.add(egui::Label::new(format!("North Y: {:.6} mm", point[1])).wrap());
        }
        ui.weak("Displayed decimals preserve source coordinates; measured accuracy comes from the registered evidence.");
        if theme::primary(ui, "Use edge as reference guide").clicked() {
            return Some(Guide { level, line });
        }
        None
    }
}
