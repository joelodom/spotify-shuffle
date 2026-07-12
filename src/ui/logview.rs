//! Activity log view.

use crate::messages::LogLevel;

use super::StudioApp;

impl StudioApp {
    pub(crate) fn view_log(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Activity Log");
            if ui.small_button("Clear").clicked() {
                self.log.clear();
            }
        });
        ui.label(
            egui::RichText::new(
                "Every operation, safety decision, and assumption the app makes at runtime \
                 is logged here.",
            )
            .weak()
            .small(),
        );
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (level, time, message) in &self.log {
                    let (color, tag) = match level {
                        LogLevel::Info => (egui::Color32::GRAY, "info"),
                        LogLevel::Success => (egui::Color32::from_rgb(30, 180, 90), " ok "),
                        LogLevel::Warn => (egui::Color32::from_rgb(220, 160, 40), "warn"),
                        LogLevel::Error => (egui::Color32::from_rgb(230, 80, 80), "FAIL"),
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.monospace(egui::RichText::new(format!("{time} [{tag}]")).color(color));
                        ui.label(message);
                    });
                }
            });
    }
}
