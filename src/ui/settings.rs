//! Settings view: Spotify app credentials, AI provider selection, and
//! connection management. Two fixed columns — no page scrolling. The
//! editable state is a draft `AppConfig`; nothing takes effect until
//! "Apply & save".

use crate::config::AiProviderKind;
use crate::messages::Command;

use super::StudioApp;

impl StudioApp {
    pub(crate) fn view_settings(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.add_space(4.0);

        ui.columns(2, |cols| {
            // ---------------- Left: Spotify ----------------
            cols[0].group(|ui| {
                ui.strong("Spotify");
                egui::Grid::new("spotify-grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Client ID:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.cfg_draft.spotify.client_id)
                                .desired_width(300.0)
                                .hint_text("from developer.spotify.com/dashboard"),
                        );
                        ui.end_row();

                        ui.label("Redirect port:");
                        ui.add(egui::DragValue::new(
                            &mut self.cfg_draft.spotify.redirect_port,
                        ));
                        ui.end_row();

                        ui.label("Redirect URI:");
                        ui.horizontal(|ui| {
                            let uri = self.cfg_draft.redirect_uri();
                            ui.monospace(&uri);
                            if ui.small_button("copy").clicked() {
                                ui.ctx().copy_text(uri);
                            }
                        });
                        ui.end_row();

                        ui.label("New playlists:");
                        ui.checkbox(
                            &mut self.cfg_draft.spotify.create_public,
                            "create as public (default: private)",
                        );
                        ui.end_row();
                    });
                ui.horizontal(|ui| {
                    let busy = self.is_busy();
                    if ui
                        .add_enabled(!busy, egui::Button::new("Connect to Spotify…"))
                        .clicked()
                    {
                        self.send(Command::Connect);
                    }
                    if ui
                        .add_enabled(
                            self.connected() && !busy,
                            egui::Button::new("Disconnect (forget tokens)"),
                        )
                        .clicked()
                    {
                        self.send(Command::Disconnect);
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "Spotify refresh tokens expire ~6 months after authorization \
                         (policy since June 2026) — reconnect when prompted. \
                         Development-mode apps require the owner to have Premium and \
                         allow at most 5 users.",
                    )
                    .weak()
                    .small(),
                );
            });

            // ---------------- Right: AI provider ----------------
            cols[1].group(|ui| {
                ui.strong("AI provider");
                if let Some(auth) = &self.auth {
                    ui.label(
                        egui::RichText::new(format!("Active: {}", auth.provider_desc)).small(),
                    );
                }
                egui::Grid::new("ai-grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Provider:");
                        egui::ComboBox::from_id_salt("ai-provider")
                            .selected_text(self.cfg_draft.ai.provider.label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.cfg_draft.ai.provider,
                                    AiProviderKind::ClaudeCode,
                                    AiProviderKind::ClaudeCode.label(),
                                );
                                ui.selectable_value(
                                    &mut self.cfg_draft.ai.provider,
                                    AiProviderKind::AnthropicApi,
                                    AiProviderKind::AnthropicApi.label(),
                                );
                            });
                        ui.end_row();

                        match self.cfg_draft.ai.provider {
                            AiProviderKind::ClaudeCode => {
                                ui.label("Model (--model):");
                                ui.add(
                                    egui::TextEdit::singleline(
                                        &mut self.cfg_draft.ai.claude_code_model,
                                    )
                                    .hint_text("empty = your CLI's default")
                                    .desired_width(260.0),
                                );
                                ui.end_row();

                                ui.label("claude binary:");
                                ui.add(
                                    egui::TextEdit::singleline(
                                        &mut self.cfg_draft.ai.claude_binary,
                                    )
                                    .hint_text("empty = auto-detect")
                                    .desired_width(260.0),
                                );
                                ui.end_row();

                                ui.label("Timeout (s):");
                                ui.add(egui::DragValue::new(
                                    &mut self.cfg_draft.ai.claude_timeout_secs,
                                ));
                                ui.end_row();
                            }
                            AiProviderKind::AnthropicApi => {
                                ui.label("Model:");
                                ui.add(
                                    egui::TextEdit::singleline(
                                        &mut self.cfg_draft.ai.anthropic_model,
                                    )
                                    .desired_width(260.0),
                                );
                                ui.end_row();

                                ui.label("API key env var:");
                                ui.add(
                                    egui::TextEdit::singleline(
                                        &mut self.cfg_draft.ai.anthropic_api_key_env,
                                    )
                                    .desired_width(260.0),
                                );
                                ui.end_row();
                            }
                        }
                    });
                match self.cfg_draft.ai.provider {
                    AiProviderKind::ClaudeCode => ui.label(
                        egui::RichText::new(
                            "Uses your existing Claude subscription via the logged-in Claude \
                             Code CLI (headless mode). No API key involved; usage counts \
                             against your plan's limits.",
                        )
                        .weak()
                        .small(),
                    ),
                    AiProviderKind::AnthropicApi => ui.label(
                        egui::RichText::new(
                            "Direct pay-per-token API access. The key is read from the \
                             environment at startup and never stored by this app.",
                        )
                        .weak()
                        .small(),
                    ),
                };
                ui.horizontal(|ui| {
                    let busy = self.is_busy();
                    if ui
                        .add_enabled(!busy, egui::Button::new("Check connection (free)"))
                        .clicked()
                    {
                        self.ai_test = None;
                        self.send(Command::CheckAi);
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Test generation (uses quota)"))
                        .clicked()
                    {
                        self.ai_test = None;
                        self.send(Command::TestAi);
                    }
                });
                if let Some((ok, message)) = &self.ai_test {
                    let color = if *ok {
                        egui::Color32::from_rgb(30, 180, 90)
                    } else {
                        egui::Color32::from_rgb(230, 80, 80)
                    };
                    ui.colored_label(color, message);
                }
            });
        });

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Apply & save settings").clicked() {
                self.send(Command::ApplyConfig(Box::new(self.cfg_draft.clone())));
            }
            ui.label(
                egui::RichText::new(format!(
                    "config: {}",
                    crate::config::config_path().display()
                ))
                .weak()
                .small(),
            );
        });
    }
}
