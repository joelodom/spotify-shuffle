//! Insights view: recently played + top artists/tracks (the listening data
//! the 2026 API still exposes).

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

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::CollapsingHeader::new(format!(
                    "Recently played ({} plays)",
                    data.recent.len()
                ))
                .default_open(true)
                .show(ui, |ui| {
                    if !data.recent_artist_counts.is_empty() {
                        let line = data
                            .recent_artist_counts
                            .iter()
                            .map(|(name, n)| format!("{name} ×{n}"))
                            .collect::<Vec<_>>()
                            .join("   ");
                        ui.label(format!("Top artists in this window: {line}"));
                        ui.add_space(4.0);
                    }
                    ui.push_id("recent-table", |ui| {
                        TableBuilder::new(ui)
                            .striped(true)
                            .column(Column::auto().at_least(130.0))
                            .column(Column::remainder().at_least(180.0))
                            .column(Column::remainder().at_least(140.0))
                            .max_scroll_height(260.0)
                            .header(20.0, |mut header| {
                                for t in ["When", "Track", "Artists"] {
                                    header.col(|ui| {
                                        ui.strong(t);
                                    });
                                }
                            })
                            .body(|body| {
                                body.rows(20.0, data.recent.len(), |mut row| {
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
                });

                egui::CollapsingHeader::new("Listening clock (plays by hour, recent window)")
                    .default_open(true)
                    .show(ui, |ui| {
                        let max = data.hour_histogram.iter().copied().max().unwrap_or(0);
                        if max == 0 {
                            ui.label(egui::RichText::new("no timestamps available").weak());
                        } else {
                            for (hour, count) in data.hour_histogram.iter().enumerate() {
                                if *count == 0 {
                                    continue;
                                }
                                let bar_len = ((*count as f32 / max as f32) * 30.0).ceil() as usize;
                                ui.monospace(format!(
                                    "{hour:02}:00  {}  {count}",
                                    "█".repeat(bar_len)
                                ));
                            }
                        }
                    });

                for top in &data.tops {
                    egui::CollapsingHeader::new(format!("Top items — {}", top.range_label))
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.columns(2, |cols| {
                                cols[0].strong("Artists");
                                for (i, a) in top.artists.iter().enumerate() {
                                    cols[0].label(format!("{}. {a}", i + 1));
                                }
                                cols[1].strong("Tracks");
                                for (i, t) in top.tracks.iter().enumerate() {
                                    cols[1].label(format!("{}. {t}", i + 1));
                                }
                            });
                        });
                }
            });
    }
}
