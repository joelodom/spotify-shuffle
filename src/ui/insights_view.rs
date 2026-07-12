//! Insights view: recently played + top artists/tracks (the listening data
//! the 2026 API still exposes).
//!
//! Fixed layout — the page itself never scrolls; each list scrolls
//! internally.

use egui_extras::{Column, TableBuilder};

use crate::messages::Command;

use super::StudioApp;

impl StudioApp {
    pub(crate) fn view_insights(&mut self, ui: &mut egui::Ui) {
        ui.heading("Listening Insights");
        ui.horizontal(|ui| {
            let can = self.connected() && !self.is_busy();
            if ui
                .add_enabled(can, egui::Button::new("⟳ Refresh insights"))
                .clicked()
            {
                self.send(Command::FetchInsights);
            }
            ui.label(
                egui::RichText::new(
                    "Spotify exposes the ~50 most recent plays plus top artists/tracks per \
                     time range — no deeper history exists in the Web API.",
                )
                .weak()
                .small(),
            );
        });
        let Some(data) = &self.insights else {
            ui.add_space(12.0);
            ui.label(egui::RichText::new("No data yet — press Refresh.").weak());
            return;
        };
        let data = data.clone();

        ui.add_space(4.0);
        ui.group(|ui| {
            ui.strong(format!("Recently played ({} plays)", data.recent.len()));
            ui.columns(2, |cols| {
                // Left: the play-by-play table (scrolls internally).
                cols[0].push_id("recent-table", |ui| {
                    TableBuilder::new(ui)
                        .striped(true)
                        .column(Column::auto().at_least(150.0))
                        .column(Column::remainder().at_least(160.0))
                        .column(Column::remainder().at_least(120.0))
                        .max_scroll_height(300.0)
                        .header(28.0, |mut header| {
                            for t in ["When", "Track", "Artists"] {
                                header.col(|ui| {
                                    ui.strong(t);
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(28.0, data.recent.len(), |mut row| {
                                let r = &data.recent[row.index()];
                                row.col(|ui| {
                                    ui.label(&r.when_local);
                                });
                                row.col(|ui| {
                                    ui.label(&r.title);
                                });
                                row.col(|ui| {
                                    ui.label(&r.artists);
                                });
                            });
                        });
                });
                // Right: window stats + listening clock.
                let col = &mut cols[1];
                if !data.recent_artist_counts.is_empty() {
                    col.strong("Top artists in this window");
                    let line = data
                        .recent_artist_counts
                        .iter()
                        .map(|(name, n)| format!("{name} ×{n}"))
                        .collect::<Vec<_>>()
                        .join("   ");
                    col.label(line);
                    col.add_space(6.0);
                }
                col.strong("Listening clock (plays by hour)");
                let max = data.hour_histogram.iter().copied().max().unwrap_or(0);
                if max == 0 {
                    col.label(egui::RichText::new("no timestamps available").weak());
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("clock-scroll")
                        .max_height(200.0)
                        .show(col, |ui| {
                            for (hour, count) in data.hour_histogram.iter().enumerate() {
                                if *count == 0 {
                                    continue;
                                }
                                let bar_len = ((*count as f32 / max as f32) * 24.0).ceil() as usize;
                                ui.monospace(format!(
                                    "{hour:02}:00  {}  {count}",
                                    "█".repeat(bar_len)
                                ));
                            }
                        });
                }
            });
        });

        ui.add_space(6.0);
        ui.strong("Top items");
        ui.columns(3, |cols| {
            for (i, top) in data.tops.iter().enumerate() {
                let col = &mut cols[i];
                col.group(|ui| {
                    ui.strong(top.range_label);
                    egui::ScrollArea::vertical()
                        .id_salt(("top-scroll", i))
                        .max_height(240.0)
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Artists").weak());
                            for (n, a) in top.artists.iter().enumerate() {
                                ui.label(format!("{}. {a}", n + 1));
                            }
                            ui.separator();
                            ui.label(egui::RichText::new("Tracks").weak());
                            for (n, t) in top.tracks.iter().enumerate() {
                                ui.label(format!("{}. {t}", n + 1));
                            }
                        });
                });
            }
        });
    }
}
