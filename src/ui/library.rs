//! Library view: playlists table (with tier badges and per-row actions) and
//! the track listing of whatever source was last loaded.

use egui_extras::{Column, TableBuilder};

use crate::messages::Command;
use crate::ops::TrackSource;
use crate::safety::Tier;

use super::{RenameDialog, StudioApp};

impl StudioApp {
    pub(crate) fn view_library(&mut self, ui: &mut egui::Ui) {
        ui.heading("Library");
        if !self.connected() {
            ui.label("Connect to Spotify to load your library (Setup Guide → step 2).");
            return;
        }
        ui.label(
            egui::RichText::new(
                "Protected = everything not created this run: read-only contents, deletion \
                 only via the guarded flow. In development mode Spotify only exposes the \
                 CONTENTS of playlists you own or collaborate on.",
            )
            .weak()
            .small(),
        );
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            let liked_selected = self.selected == Some(TrackSource::LikedSongs);
            if ui
                .selectable_label(liked_selected, "♡ Liked Songs (source)")
                .clicked()
            {
                self.selected = Some(TrackSource::LikedSongs);
            }
            let enabled = !self.is_busy();
            if ui
                .add_enabled(enabled, egui::Button::new("Load Liked Songs"))
                .clicked()
            {
                self.send(Command::LoadTracks(TrackSource::LikedSongs));
            }
            ui.separator();
            ui.label(format!(
                "Selected source: {}",
                self.selected
                    .as_ref()
                    .map(|s| s.label())
                    .unwrap_or_else(|| "—".into())
            ));
        });
        ui.add_space(4.0);

        let mut to_send: Vec<Command> = Vec::new();
        let mut new_selected: Option<TrackSource> = None;
        let mut rename: Option<RenameDialog> = None;
        let busy = self.is_busy();

        ui.push_id("playlists-table", |ui| {
            TableBuilder::new(ui)
                .striped(true)
                .column(Column::remainder().at_least(180.0)) // name
                .column(Column::auto().at_least(90.0)) // owner
                .column(Column::auto().at_least(50.0)) // tracks
                .column(Column::auto().at_least(70.0)) // tier
                .column(Column::auto().at_least(210.0)) // actions
                .max_scroll_height(300.0)
                .header(20.0, |mut header| {
                    header.col(|ui| {
                        ui.strong(format!("Playlist ({})", self.playlists.len()));
                    });
                    header.col(|ui| {
                        ui.strong("Owner");
                    });
                    header.col(|ui| {
                        ui.strong("Tracks");
                    });
                    header.col(|ui| {
                        ui.strong("Tier");
                    });
                    header.col(|ui| {
                        ui.strong("Actions");
                    });
                })
                .body(|body| {
                    body.rows(22.0, self.playlists.len(), |mut row| {
                        let p = &self.playlists[row.index()];
                        let source = TrackSource::Playlist {
                            id: p.id.clone(),
                            name: p.name.clone(),
                        };
                        row.col(|ui| {
                            let is_selected = self.selected.as_ref() == Some(&source);
                            let label = if p.readable {
                                p.name.clone()
                            } else {
                                format!("{} (contents not readable)", p.name)
                            };
                            if ui.selectable_label(is_selected, label).clicked() {
                                new_selected = Some(source.clone());
                            }
                        });
                        row.col(|ui| {
                            ui.label(&p.owner);
                        });
                        row.col(|ui| {
                            ui.label(p.total.to_string());
                        });
                        row.col(|ui| match p.tier {
                            Tier::Session => {
                                ui.colored_label(egui::Color32::from_rgb(30, 180, 90), "session");
                            }
                            Tier::Protected => {
                                ui.colored_label(egui::Color32::GRAY, "protected");
                            }
                        });
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        !busy && p.readable,
                                        egui::Button::new("Load").small(),
                                    )
                                    .clicked()
                                {
                                    to_send.push(Command::LoadTracks(source.clone()));
                                }
                                if p.tier == Tier::Session {
                                    if ui
                                        .add_enabled(!busy, egui::Button::new("Rename").small())
                                        .clicked()
                                    {
                                        rename = Some(RenameDialog {
                                            id: p.id.clone(),
                                            old_name: p.name.clone(),
                                            buffer: p.name.clone(),
                                        });
                                    }
                                    let delete = egui::Button::new(
                                        egui::RichText::new("Delete").color(egui::Color32::WHITE),
                                    )
                                    .small()
                                    .fill(egui::Color32::from_rgb(160, 60, 60));
                                    if ui.add_enabled(!busy, delete).clicked() {
                                        to_send.push(Command::DeleteSession {
                                            id: p.id.clone(),
                                            name: p.name.clone(),
                                        });
                                    }
                                } else {
                                    let delete = egui::Button::new("Delete…").small();
                                    if ui
                                        .add_enabled(!busy, delete)
                                        .on_hover_text(
                                            "Protected playlist — opens the guarded \
                                             confirmation flow",
                                        )
                                        .clicked()
                                    {
                                        to_send.push(Command::ArmGuardedDelete {
                                            id: p.id.clone(),
                                            name: p.name.clone(),
                                        });
                                    }
                                }
                            });
                        });
                    });
                });
        });

        if let Some(sel) = new_selected {
            self.selected = Some(sel);
        }
        if let Some(r) = rename {
            self.rename_dialog = Some(r);
        }
        for cmd in to_send {
            self.send(cmd);
        }

        ui.add_space(8.0);
        ui.separator();
        match &self.tracks {
            None => {
                ui.label(
                    egui::RichText::new(
                        "No tracks loaded yet — press “Load” on a playlist or Liked Songs.",
                    )
                    .weak(),
                );
            }
            Some((label, rows)) => {
                ui.strong(format!("Tracks — {label} ({})", rows.len()));
                ui.push_id("tracks-table", |ui| {
                    TableBuilder::new(ui)
                        .striped(true)
                        .column(Column::auto().at_least(34.0)) // #
                        .column(Column::remainder().at_least(160.0)) // title
                        .column(Column::remainder().at_least(120.0)) // artists
                        .column(Column::remainder().at_least(120.0)) // album
                        .column(Column::auto().at_least(46.0)) // duration
                        .column(Column::auto().at_least(74.0)) // released
                        .column(Column::auto().at_least(74.0)) // added
                        .header(20.0, |mut header| {
                            for title in
                                ["#", "Title", "Artists", "Album", "Len", "Released", "Added"]
                            {
                                header.col(|ui| {
                                    ui.strong(title);
                                });
                            }
                        })
                        .body(|body| {
                            body.rows(20.0, rows.len(), |mut row| {
                                let i = row.index();
                                let t = &rows[i];
                                row.col(|ui| {
                                    ui.label((i + 1).to_string());
                                });
                                row.col(|ui| {
                                    if t.is_local {
                                        ui.label(format!("{} (local)", t.title));
                                    } else if t.is_episode {
                                        ui.label(format!("{} (episode)", t.title));
                                    } else {
                                        ui.label(&t.title);
                                    }
                                });
                                row.col(|ui| {
                                    ui.label(&t.artists);
                                });
                                row.col(|ui| {
                                    ui.label(&t.album);
                                });
                                row.col(|ui| {
                                    ui.label(&t.duration);
                                });
                                row.col(|ui| {
                                    ui.label(&t.release_date);
                                });
                                row.col(|ui| {
                                    ui.label(&t.added_at);
                                });
                            });
                        });
                });
            }
        }
    }
}
