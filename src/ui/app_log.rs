use eframe::egui;

impl super::App {
    pub(super) fn draw_app_log_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Clear").clicked() {
                self.log_entries.clear();
            }
            ui.separator();
            ui.label("Filter:");
            ui.text_edit_singleline(&mut self.log_filter);
            ui.separator();
            ui.checkbox(&mut self.log_auto_scroll, "Auto-scroll");

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("{} entries", self.log_entries.len()));
            });
        });

        ui.separator();

        let filter_lower = self.log_filter.to_lowercase();

        egui::ScrollArea::vertical()
            .stick_to_bottom(self.log_auto_scroll)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.style_mut().override_font_id = Some(egui::FontId::monospace(12.0));
                for entry in &self.log_entries {
                    let message = self.display_text(&entry.message);
                    if !filter_lower.is_empty() && !message.to_lowercase().contains(&filter_lower) {
                        continue;
                    }
                    let color = entry.level.color();
                    let line = format!("{} [{}] {}", entry.timestamp, entry.level.label(), message);
                    ui.label(egui::RichText::new(line).color(color));
                }
            });
    }
}
