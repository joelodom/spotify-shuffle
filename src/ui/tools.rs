//! Tools view: unbiased shuffle, dedupe, sort, merge, import, export.
//!
//! Laid out in two fixed columns — no page scrolling (the merge source list
//! scrolls internally).

use crate::messages::Command;
use crate::ops::TrackSource;
use crate::ops::import_export::ExportFormat;
use crate::ops::playlist_tools::SortKey;

use super::StudioApp;

impl StudioApp {
    pub(crate) fn view_tools(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tools");
        let ready = self.connected() && !self.is_busy();
        if !self.connected() {
            ui.label("Connect to Spotify first (Setup Guide).");
        }

        ui.horizontal(|ui| {
            ui.label("Working source:");
            self.source_picker_in(ui, "tools-source");
            let is_session = self.selected_is_session(&self.selected.clone());
            if is_session {
                ui.checkbox(&mut self.tools_in_place, "Edit in place (session playlist)");
            } else {
                self.tools_in_place = false;
                ui.label(
                    egui::RichText::new("results go to a NEW playlist")
                        .weak()
                        .small(),
                );
            }
        });
        ui.add_space(6.0);

        ui.columns(2, |cols| {
            // ---------------- Left column ----------------
            cols[0].group(|ui| {
                ui.strong("Unbiased shuffle");
                ui.label(
                    egui::RichText::new(
                        "Fisher–Yates with a ChaCha20 CSPRNG — every ordering equally \
                         likely, unlike Spotify's own shuffle. Always writes a NEW \
                         playlist; perfect for Liked Songs.",
                    )
                    .weak()
                    .small(),
                );
                let can = ready && self.selected.is_some();
                if ui
                    .add_enabled(can, egui::Button::new("🔀 Create shuffled playlist"))
                    .clicked()
                    && let Some(source) = self.selected.clone()
                {
                    self.send(Command::Shuffle { source });
                }
            });
            cols[0].add_space(8.0);

            cols[0].group(|ui| {
                ui.strong("Remove duplicates");
                ui.label(
                    egui::RichText::new(
                        "Finds exact repeats and same-song-different-edition repeats \
                         (normalized title + artist). First occurrence wins.",
                    )
                    .weak()
                    .small(),
                );
                let can = ready && self.selected.is_some();
                if ui.add_enabled(can, egui::Button::new("♻ Dedupe")).clicked()
                    && let Some(source) = self.selected.clone()
                {
                    self.send(Command::Dedupe {
                        source,
                        in_place: self.tools_in_place,
                    });
                }
            });
            cols[0].add_space(8.0);

            cols[0].group(|ui| {
                ui.strong("Sort");
                ui.label(
                    egui::RichText::new(
                        "Metadata sorts only — Spotify removed audio features (BPM/energy) \
                         for new apps in Nov 2024.",
                    )
                    .weak()
                    .small(),
                );
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("sort-key")
                        .selected_text(self.sort_key.label())
                        .show_ui(ui, |ui| {
                            for key in SortKey::ALL {
                                ui.selectable_value(&mut self.sort_key, key, key.label());
                            }
                        });
                    ui.checkbox(&mut self.sort_desc, "descending");
                    let can = ready && self.selected.is_some();
                    if ui.add_enabled(can, egui::Button::new("⇅ Sort")).clicked()
                        && let Some(source) = self.selected.clone()
                    {
                        self.send(Command::Sort {
                            source,
                            key: self.sort_key,
                            descending: self.sort_desc,
                            in_place: self.tools_in_place,
                        });
                    }
                });
            });
            cols[0].add_space(8.0);

            cols[0].group(|ui| {
                ui.strong("Export / backup");
                ui.label(
                    egui::RichText::new(
                        "Save the working source as CSV or JSON — backups survive anything \
                         Spotify does to your account.",
                    )
                    .weak()
                    .small(),
                );
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("export-format")
                        .selected_text(match self.export_format {
                            ExportFormat::Csv => "CSV",
                            ExportFormat::Json => "JSON",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.export_format, ExportFormat::Csv, "CSV");
                            ui.selectable_value(
                                &mut self.export_format,
                                ExportFormat::Json,
                                "JSON",
                            );
                        });
                    let can = ready && self.selected.is_some();
                    if ui
                        .add_enabled(can, egui::Button::new("💾 Export…"))
                        .clicked()
                        && let Some(source) = self.selected.clone()
                    {
                        let ext = match self.export_format {
                            ExportFormat::Csv => "csv",
                            ExportFormat::Json => "json",
                        };
                        let default_name =
                            format!("{}.{ext}", source.label().replace(['/', ':'], "-"));
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("Export tracks")
                            .set_file_name(default_name)
                            .save_file()
                        {
                            self.send(Command::Export {
                                source,
                                format: self.export_format,
                                path,
                            });
                        }
                    }
                });
            });

            // ---------------- Right column ----------------
            cols[1].group(|ui| {
                ui.strong("Merge sources into a new playlist");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.merge_dedupe, "dedupe");
                    ui.checkbox(&mut self.merge_shuffle, "shuffle result");
                });
                egui::ScrollArea::vertical()
                    .id_salt("merge-list")
                    .max_height(190.0)
                    .show(ui, |ui| {
                        let toggle = |ui: &mut egui::Ui,
                                      list: &mut Vec<TrackSource>,
                                      source: TrackSource,
                                      label: String| {
                            let mut checked = list.contains(&source);
                            if ui.checkbox(&mut checked, label).changed() {
                                if checked {
                                    list.push(source);
                                } else {
                                    list.retain(|s| s != &source);
                                }
                            }
                        };
                        toggle(
                            ui,
                            &mut self.merge_selected,
                            TrackSource::LikedSongs,
                            "Liked Songs".into(),
                        );
                        let rows: Vec<(TrackSource, String)> = self
                            .playlists
                            .iter()
                            .filter(|p| p.readable)
                            .map(|p| {
                                (
                                    TrackSource::Playlist {
                                        id: p.id.clone(),
                                        name: p.name.clone(),
                                    },
                                    format!("{} ({} tracks)", p.name, p.total),
                                )
                            })
                            .collect();
                        for (source, label) in rows {
                            toggle(ui, &mut self.merge_selected, source, label);
                        }
                    });
                let can = ready && self.merge_selected.len() >= 2;
                if ui
                    .add_enabled(
                        can,
                        egui::Button::new(format!("⧉ Merge {} sources", self.merge_selected.len())),
                    )
                    .clicked()
                {
                    self.send(Command::Merge {
                        sources: self.merge_selected.clone(),
                        dedupe: self.merge_dedupe,
                        shuffle: self.merge_shuffle,
                    });
                }
            });
            cols[1].add_space(8.0);

            cols[1].group(|ui| {
                ui.strong("Import a pasted track list → new playlist");
                ui.label(
                    egui::RichText::new("One `Artist - Title` per line (also accepts — or TAB).")
                        .weak()
                        .small(),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.import_name)
                        .hint_text("New playlist name"),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut self.import_text)
                        .hint_text("Daft Punk - Around the World\nBjörk - Hyperballad\n…")
                        .desired_rows(5)
                        .desired_width(f32::INFINITY),
                );
                let can = ready && !self.import_text.trim().is_empty();
                if ui.add_enabled(can, egui::Button::new("⤵ Import")).clicked() {
                    self.send(Command::Import {
                        name: self.import_name.clone(),
                        text: self.import_text.clone(),
                    });
                }
            });
        });
    }
}
