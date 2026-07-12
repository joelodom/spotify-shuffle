//! Setup Guide — a first-run wizard that walks through the two connections
//! the app needs: a personal Spotify app (Client ID + OAuth) and Claude
//! (the already-logged-in Claude Code CLI, verified without spending quota).

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
        ui.label(egui::RichText::new(mark).color(color).size(20.0));
        ui.label(egui::RichText::new(title).strong().size(18.0));
    });
}

impl StudioApp {
    pub(crate) fn view_setup(&mut self, ui: &mut egui::Ui) {
        ui.heading("Setup Guide");
        ui.label(
            "Two one-time connections and you're done. Everything here stays on this \
             machine: the Client ID and OAuth tokens live in your local config directory.",
        );
        ui.add_space(8.0);

        let client_id_set = !self.cfg_draft.spotify.client_id.trim().is_empty();
        let connected = self.connected();
        let ai_ok = self.ai_test.as_ref().map(|(ok, _)| *ok).unwrap_or(false);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // ---------------- Step 1 ----------------
                ui.group(|ui| {
                    step_header(ui, client_id_set, "Step 1 — Register your own Spotify app");
                    ui.label(
                        "Spotify requires every user of the Web API to register an app. Yours \
                     will run in \"Development Mode\": private to you, which is exactly what \
                     this tool needs.",
                    );
                    ui.add_space(4.0);
                    ui.label("1. Open the Spotify developer dashboard and log in:");
                    ui.hyperlink_to(format!("   {DASHBOARD_URL}"), DASHBOARD_URL);
                    ui.label("2. Press “Create app”. Name and description can be anything.");
                    ui.horizontal_wrapped(|ui| {
                        ui.label("3. In “Redirect URIs” add EXACTLY:");
                        let uri = self.cfg_draft.redirect_uri();
                        ui.monospace(&uri);
                        if ui.small_button("copy").clicked() {
                            ui.ctx().copy_text(uri);
                        }
                    });
                    ui.label(
                        "   (Must be the 127.0.0.1 IP form — Spotify banned the word \
                     “localhost” in redirect URIs in 2025.)",
                    );
                    ui.label(
                        "4. Under “Which API/SDKs are you planning to use?” tick Web API. Save.",
                    );
                    ui.label("5. Open the app's Settings page and copy its Client ID here:");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.cfg_draft.spotify.client_id)
                                .hint_text("paste Client ID (32 hex characters)")
                                .desired_width(340.0),
                        );
                        if ui.button("Save Client ID").clicked() {
                            self.send(Command::ApplyConfig(Box::new(self.cfg_draft.clone())));
                        }
                    });
                    ui.label(
                        egui::RichText::new(
                            "Requirements (Spotify policy since Feb 2026): the account that owns \
                         the app must have Premium; one development-mode app per account; up \
                         to 5 allowlisted users. Using it just for yourself — as here — is \
                         the intended case. There is no client secret: this app uses the \
                         PKCE flow.",
                        )
                        .weak()
                        .small(),
                    );
                });
                ui.add_space(8.0);

                // ---------------- Step 2 ----------------
                ui.group(|ui| {
                    step_header(ui, connected, "Step 2 — Connect your Spotify account");
                    ui.label(
                        "This opens Spotify in your browser to authorize the app (OAuth with \
                     PKCE). Approve it, and the browser tab will say you can return here. \
                     Tokens are saved locally, so this survives restarts — Spotify does \
                     force a fresh login roughly every 6 months.",
                    );
                    ui.horizontal(|ui| {
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
                    });
                    ui.label(
                        egui::RichText::new(
                            "If the browser shows INVALID_CLIENT: the redirect URI in the \
                         dashboard doesn't byte-match the one above (check port), or the \
                         Client ID is wrong.",
                        )
                        .weak()
                        .small(),
                    );
                });
                ui.add_space(8.0);

                // ---------------- Step 3 ----------------
                ui.group(|ui| {
                    step_header(ui, ai_ok, "Step 3 — Connect Claude (your subscription)");
                    ui.label(
                        "The AI features run through the Claude Code CLI you already have \
                     installed and logged in — headless, on your existing subscription's \
                     OAuth token. No API key, no extra cost beyond your plan.",
                    );
                    ui.horizontal(|ui| {
                        let can = !self.is_busy();
                        if ui
                            .add_enabled(
                                can,
                                egui::Button::new("🤖 Check Claude connection (free)"),
                            )
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
                    });
                    ui.add_space(4.0);
                    ui.label("If the check fails:");
                    ui.label("• “not found” — install Claude Code, then restart this app:");
                    ui.hyperlink_to(
                        "   https://claude.com/claude-code",
                        "https://claude.com/claude-code",
                    );
                    ui.label(
                        "• “not logged in” — open a terminal, run `claude`, type /login and \
                     sign in with your Claude account, then check again. Your login is \
                     stored by Claude Code itself; this app simply reuses it.",
                    );
                    ui.label(
                        egui::RichText::new(
                            "Prefer pay-per-token API billing instead? Switch the provider in \
                         Settings → AI provider and export an ANTHROPIC_API_KEY. The app is \
                         built so the two are interchangeable.",
                        )
                        .weak()
                        .small(),
                    );
                });
                ui.add_space(8.0);

                if client_id_set && connected && ai_ok {
                    ui.group(|ui| {
                        ui.label(
                            egui::RichText::new("✔ All set!")
                                .color(egui::Color32::from_rgb(30, 180, 90))
                                .strong(),
                        );
                        ui.horizontal(|ui| {
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
            });
    }
}
