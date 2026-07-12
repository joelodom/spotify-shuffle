//! Setup Guide — a first-run wizard that walks through the two connections
//! the app needs: a personal Spotify app (Client ID + OAuth) and Claude
//! (the already-logged-in Claude Code CLI, verified without spending quota).
//!
//! The three steps sit side by side — no page scrolling.

use crate::messages::Command;

use super::StudioApp;

const DASHBOARD_URL: &str = "https://developer.spotify.com/dashboard";

fn step_header(ui: &mut egui::Ui, done: bool, title: &str) {
    let (mark, color) = if done {
        ("✔", egui::Color32::from_rgb(30, 180, 90))
    } else {
        ("○", egui::Color32::from_gray(180))
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(mark).color(color).size(22.0));
        ui.label(egui::RichText::new(title).strong().size(20.0));
    });
}

impl StudioApp {
    pub(crate) fn view_setup(&mut self, ui: &mut egui::Ui) {
        ui.heading("Setup Guide");
        ui.label(
            "Two one-time connections and you're done. Everything stays on this machine: \
             the Client ID and OAuth tokens live in your local config directory.",
        );
        ui.add_space(6.0);

        let client_id_set = !self.cfg_draft.spotify.client_id.trim().is_empty();
        let connected = self.connected();
        let ai_ok = self.ai_test.as_ref().map(|(ok, _)| *ok).unwrap_or(false);

        ui.columns(3, |cols| {
            // ---------------- Step 1 ----------------
            cols[0].group(|ui| {
                step_header(ui, client_id_set, "1 — Register a Spotify app");
                ui.label(
                    "Spotify requires every Web API user to register an app. Yours runs in \
                     \"Development Mode\": private to you — exactly what this tool needs.",
                );
                ui.add_space(4.0);
                ui.label("1. Open the developer dashboard:");
                ui.hyperlink_to("developer.spotify.com/dashboard", DASHBOARD_URL);
                ui.label("2. Press “Create app” — name and description can be anything.");
                ui.label("3. Add EXACTLY this Redirect URI:");
                ui.horizontal_wrapped(|ui| {
                    let uri = self.cfg_draft.redirect_uri();
                    ui.monospace(&uri);
                    if ui.small_button("copy").clicked() {
                        ui.ctx().copy_text(uri);
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "(Must be the 127.0.0.1 form — Spotify banned “localhost” in 2025.)",
                    )
                    .weak()
                    .small(),
                );
                ui.label("4. Tick Web API under the APIs question and save.");
                ui.label("5. Copy the app's Client ID here:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.cfg_draft.spotify.client_id)
                        .hint_text("paste Client ID")
                        .desired_width(f32::INFINITY),
                );
                if ui.button("Save Client ID").clicked() {
                    self.send(Command::ApplyConfig(Box::new(self.cfg_draft.clone())));
                }
                ui.label(
                    egui::RichText::new(
                        "Spotify policy (Feb 2026): the app owner needs Premium; one \
                         development-mode app per account; up to 5 allowlisted users. \
                         Personal use — as here — is the intended case. No client secret: \
                         this app uses the PKCE flow.",
                    )
                    .weak()
                    .small(),
                );
            });

            // ---------------- Step 2 ----------------
            cols[1].group(|ui| {
                step_header(ui, connected, "2 — Connect your account");
                ui.label(
                    "Opens Spotify in your browser to authorize the app (OAuth + PKCE). \
                     Approve it and return here. Tokens are saved locally, so this \
                     survives restarts — Spotify forces a fresh login roughly every 6 \
                     months.",
                );
                ui.add_space(4.0);
                let can = client_id_set && !self.is_busy();
                if ui
                    .add_enabled(can, egui::Button::new("🔗 Connect to Spotify…"))
                    .clicked()
                {
                    self.send(Command::Connect);
                }
                if connected {
                    let user = self
                        .auth
                        .as_ref()
                        .and_then(|a| a.user.clone())
                        .unwrap_or_default();
                    ui.colored_label(
                        egui::Color32::from_rgb(30, 180, 90),
                        format!("connected as {user}"),
                    );
                } else if !client_id_set {
                    ui.label(egui::RichText::new("finish step 1 first").weak());
                }
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(
                        "If the browser shows INVALID_CLIENT: the redirect URI in the \
                         dashboard doesn't byte-match the one in step 1 (check the port), \
                         or the Client ID is wrong.",
                    )
                    .weak()
                    .small(),
                );
            });

            // ---------------- Step 3 ----------------
            cols[2].group(|ui| {
                step_header(ui, ai_ok, "3 — Connect Claude");
                ui.label(
                    "AI features run through the Claude Code CLI you already have \
                     installed and logged in — headless, on your existing subscription's \
                     OAuth token. No API key, no cost beyond your plan.",
                );
                ui.add_space(4.0);
                let can = !self.is_busy();
                if ui
                    .add_enabled(can, egui::Button::new("🤖 Check Claude connection (free)"))
                    .clicked()
                {
                    self.ai_test = None;
                    self.send(Command::CheckAi);
                }
                if let Some((ok, message)) = &self.ai_test {
                    let color = if *ok {
                        egui::Color32::from_rgb(30, 180, 90)
                    } else {
                        egui::Color32::from_rgb(230, 80, 80)
                    };
                    ui.colored_label(color, message);
                }
                ui.add_space(4.0);
                ui.label("If the check fails:");
                ui.label("• “not found” — install Claude Code, then restart this app:");
                ui.hyperlink_to("claude.com/claude-code", "https://claude.com/claude-code");
                ui.label(
                    "• “not logged in” — run `claude` in a terminal, type /login, sign \
                     in, and check again. This app simply reuses that login.",
                );
                ui.label(
                    egui::RichText::new(
                        "Prefer pay-per-token API billing? Switch the provider in \
                         Settings → AI provider and export an ANTHROPIC_API_KEY.",
                    )
                    .weak()
                    .small(),
                );
            });
        });

        if client_id_set && connected && ai_ok {
            ui.add_space(8.0);
            ui.group(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("✔ All set!")
                            .color(egui::Color32::from_rgb(30, 180, 90))
                            .strong(),
                    );
                    ui.label("Try the");
                    if ui.link("AI Studio").clicked() {
                        self.view = super::View::AiStudio;
                    }
                    ui.label("to create your first playlist, or the");
                    if ui.link("Tools").clicked() {
                        self.view = super::View::Tools;
                    }
                    ui.label("for an unbiased shuffle of your Liked Songs.");
                });
            });
        }
    }
}
