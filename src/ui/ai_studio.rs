//! AI Studio: generation, refinement, and learn-from-library reorganization.
//!
//! Laid out in two fixed columns — no page scrolling.

use crate::messages::Command;
use crate::ops::TrackSource;

use super::StudioApp;

impl StudioApp {
    pub(crate) fn view_ai_studio(&mut self, ui: &mut egui::Ui) {
        ui.heading("AI Studio");
        let ready = self.connected() && !self.is_busy();
        if !self.connected() {
            ui.label("Connect to Spotify first (Setup Guide).");
        }
        ui.add_space(4.0);

        ui.columns(2, |cols| {
            // ---------------- Left: Create ----------------
            cols[0].group(|ui| {
                ui.strong("Create a playlist from a description");
                ui.label(
                    egui::RichText::new(
                        "Claude proposes tracks; each one is verified against Spotify search \
                         before the playlist is created. (Spotify retired its own \
                         recommendation endpoints for new apps in Nov 2024 — the AI is the \
                         recommendation engine here.)",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut self.gen_desc)
                        .hint_text(
                            "e.g. \"1980s hits that sound like 2010 indie\" or \"epic rock \
                             anthems with female vocalists\"",
                        )
                        .desired_rows(5)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    ui.label("Tracks:");
                    ui.add(egui::Slider::new(&mut self.gen_count, 5..=100));
                });
                ui.checkbox(&mut self.gen_personalize, "Personalize with my top artists");
                let can = ready && !self.gen_desc.trim().is_empty();
                if ui
                    .add_enabled(can, egui::Button::new("✨ Create with AI"))
                    .clicked()
                {
                    self.send(Command::Generate {
                        description: self.gen_desc.trim().to_string(),
                        count: self.gen_count,
                        personalize: self.gen_personalize,
                    });
                }
            });

            // ---------------- Right: Refine + Organize ----------------
            let selected = self.selected.clone();
            cols[1].group(|ui| {
                ui.strong("Refine (\"more like this, less like that\")");
                ui.horizontal(|ui| {
                    ui.label("Target:");
                    self.source_picker_in(ui, "refine-source");
                });
                let is_session = self.selected_is_session(&selected);
                match &selected {
                    Some(TrackSource::Playlist { .. }) if is_session => {
                        ui.checkbox(
                            &mut self.refine_in_place,
                            "Edit in place (allowed: created this session)",
                        );
                    }
                    Some(_) => {
                        self.refine_in_place = false;
                        ui.label(
                            egui::RichText::new(
                                "Protected source — the result is written to a NEW playlist; \
                                 the original is never touched.",
                            )
                            .weak()
                            .small(),
                        );
                    }
                    None => {}
                }
                ui.add(
                    egui::TextEdit::multiline(&mut self.refine_instruction)
                        .hint_text(
                            "e.g. \"more synth-heavy and upbeat, drop the ballads, add a few \
                             deep cuts\"",
                        )
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                let can = ready && selected.is_some() && !self.refine_instruction.trim().is_empty();
                if ui
                    .add_enabled(can, egui::Button::new("🎯 Refine with AI"))
                    .clicked()
                    && let Some(source) = selected.clone()
                {
                    self.send(Command::Refine {
                        source,
                        instruction: self.refine_instruction.trim().to_string(),
                        in_place: self.refine_in_place,
                    });
                }
            });

            cols[1].add_space(8.0);

            cols[1].group(|ui| {
                ui.strong("Learn from my library");
                ui.label(
                    egui::RichText::new(
                        "Samples your playlists (owned ones), Liked Songs and top artists, \
                         then designs new playlists from what it learns. Originals are left \
                         untouched.",
                    )
                    .weak()
                    .small(),
                );
                ui.add(
                    egui::TextEdit::multiline(&mut self.organize_goal)
                        .hint_text(
                            "e.g. \"reorganize my library into coherent mood playlists\" or \
                             \"build better workout / focus / dinner playlists from what I \
                             already like\"",
                        )
                        .desired_rows(2)
                        .desired_width(f32::INFINITY),
                );
                ui.horizontal(|ui| {
                    ui.label("Max new playlists:");
                    ui.add(egui::Slider::new(&mut self.organize_max, 1..=10));
                });
                let can = ready && !self.organize_goal.trim().is_empty();
                if ui
                    .add_enabled(can, egui::Button::new("🧭 Organize with AI"))
                    .clicked()
                {
                    self.send(Command::Organize {
                        goal: self.organize_goal.trim().to_string(),
                        max_playlists: self.organize_max,
                    });
                }
            });
        });
    }

    /// Shared combo box for choosing the working source (Liked Songs or any
    /// playlist). Bound to `self.selected`.
    pub(crate) fn source_picker_in(&mut self, ui: &mut egui::Ui, salt: &str) {
        let current = self
            .selected
            .as_ref()
            .map(|s| s.label())
            .unwrap_or_else(|| "choose…".into());
        egui::ComboBox::from_id_salt(salt)
            .selected_text(current)
            .width(300.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.selected,
                    Some(TrackSource::LikedSongs),
                    "Liked Songs",
                );
                for p in &self.playlists {
                    if !p.readable {
                        continue;
                    }
                    let source = TrackSource::Playlist {
                        id: p.id.clone(),
                        name: p.name.clone(),
                    };
                    ui.selectable_value(
                        &mut self.selected,
                        Some(source),
                        format!("{} ({})", p.name, p.tier.label()),
                    );
                }
            });
    }
}
