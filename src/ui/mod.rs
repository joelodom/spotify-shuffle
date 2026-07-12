//! egui/eframe UI shell.
//!
//! The UI is a pure renderer: state mutations happen by draining worker
//! [`Event`]s each frame; anything touching Spotify or the AI goes out as a
//! [`Command`]. The guarded-deletion dialog lives here, but the typed text is
//! validated by the worker's `SafetyPolicy` — the UI cannot bypass it.

mod ai_studio;
mod insights_view;
mod library;
mod logview;
mod settings;
mod setup;
mod tools;

use std::sync::mpsc::Receiver;

use tokio::sync::mpsc::UnboundedSender;

use crate::config::AppConfig;
use crate::messages::{AuthInfo, Command, Event, LogLevel, PlaylistRow, TrackRow};
use crate::ops::TrackSource;
use crate::ops::import_export::ExportFormat;
use crate::ops::insights::InsightsData;
use crate::ops::playlist_tools::SortKey;
use crate::safety::{DELETE_CONFIRMATION_WORD, PlaylistId, Tier};
use crate::worker;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum View {
    Setup,
    Library,
    AiStudio,
    Tools,
    Insights,
    Log,
    Settings,
}

pub(crate) struct Busy {
    pub label: String,
    pub progress: Option<(u64, Option<u64>)>,
}

pub(crate) struct DeleteDialog {
    pub id: PlaylistId,
    pub name: String,
    pub input: String,
}

pub(crate) struct RenameDialog {
    pub id: PlaylistId,
    pub old_name: String,
    pub buffer: String,
}

pub struct StudioApp {
    pub(crate) cmd: UnboundedSender<Command>,
    pub(crate) events: Receiver<Event>,

    pub(crate) view: View,
    pub(crate) auth: Option<AuthInfo>,
    pub(crate) playlists: Vec<PlaylistRow>,
    pub(crate) session: Vec<(PlaylistId, String)>,
    pub(crate) tracks: Option<(String, Vec<TrackRow>)>,
    pub(crate) insights: Option<Box<InsightsData>>,
    pub(crate) busy: Option<Busy>,
    pub(crate) log: Vec<(LogLevel, String, String)>,
    pub(crate) delete_dialog: Option<DeleteDialog>,
    pub(crate) rename_dialog: Option<RenameDialog>,

    // Shared source selection (Library ⇄ AI Studio ⇄ Tools).
    pub(crate) selected: Option<TrackSource>,

    // AI Studio forms.
    pub(crate) gen_desc: String,
    pub(crate) gen_count: usize,
    pub(crate) gen_personalize: bool,
    pub(crate) refine_instruction: String,
    pub(crate) refine_in_place: bool,
    pub(crate) organize_goal: String,
    pub(crate) organize_max: usize,

    // Tools forms.
    pub(crate) merge_selected: Vec<TrackSource>,
    pub(crate) merge_dedupe: bool,
    pub(crate) merge_shuffle: bool,
    pub(crate) sort_key: SortKey,
    pub(crate) sort_desc: bool,
    pub(crate) tools_in_place: bool,
    pub(crate) import_name: String,
    pub(crate) import_text: String,
    pub(crate) export_format: ExportFormat,

    // Settings.
    pub(crate) cfg_draft: AppConfig,
    pub(crate) ai_test: Option<(bool, String)>,
}

impl StudioApp {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: AppConfig) -> Self {
        let (cmd, events) = worker::spawn(cfg.clone(), cc.egui_ctx.clone());
        cc.egui_ctx.all_styles_mut(|style| {
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        });
        // Land new users in the wizard; returning users go straight to the
        // library.
        let start_view = if cfg.spotify.client_id.trim().is_empty() {
            View::Setup
        } else {
            View::Library
        };
        Self {
            cmd,
            events,
            view: start_view,
            auth: None,
            playlists: Vec::new(),
            session: Vec::new(),
            tracks: None,
            insights: None,
            busy: None,
            log: Vec::new(),
            delete_dialog: None,
            rename_dialog: None,
            selected: Some(TrackSource::LikedSongs),
            gen_desc: String::new(),
            gen_count: 25,
            gen_personalize: true,
            refine_instruction: String::new(),
            refine_in_place: false,
            organize_goal: String::new(),
            organize_max: 5,
            merge_selected: Vec::new(),
            merge_dedupe: true,
            merge_shuffle: false,
            sort_key: SortKey::Artist,
            sort_desc: false,
            tools_in_place: false,
            import_name: String::new(),
            import_text: String::new(),
            export_format: ExportFormat::Csv,
            cfg_draft: cfg,
            ai_test: None,
        }
    }

    pub(crate) fn send(&self, cmd: Command) {
        let _ = self.cmd.send(cmd);
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    pub(crate) fn connected(&self) -> bool {
        self.auth.as_ref().map(|a| a.connected).unwrap_or(false)
    }

    pub(crate) fn tier_of(&self, source: &TrackSource) -> Option<Tier> {
        match source {
            TrackSource::LikedSongs => None,
            TrackSource::Playlist { id, .. } => self
                .playlists
                .iter()
                .find(|p| &p.id == id)
                .map(|p| p.tier)
                .or(Some(Tier::Protected)),
        }
    }

    /// True only when the source is a playlist created this session — the
    /// only case in-place editing is offered. (The worker re-validates.)
    pub(crate) fn selected_is_session(&self, source: &Option<TrackSource>) -> bool {
        source
            .as_ref()
            .and_then(|s| self.tier_of(s))
            .map(|t| t == Tier::Session)
            .unwrap_or(false)
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Log(level, time, message) => {
                    self.log.push((level, time, message));
                    if self.log.len() > 800 {
                        self.log.drain(..200);
                    }
                }
                Event::Auth(info) => self.auth = Some(info),
                Event::Playlists(rows) => {
                    // Keep selections valid across refreshes.
                    let still_exists = |s: &TrackSource| match s {
                        TrackSource::LikedSongs => true,
                        TrackSource::Playlist { id, .. } => rows.iter().any(|p| &p.id == id),
                    };
                    if let Some(sel) = &self.selected
                        && !still_exists(sel)
                    {
                        self.selected = Some(TrackSource::LikedSongs);
                    }
                    self.merge_selected.retain(|s| still_exists(s));
                    self.playlists = rows;
                }
                Event::SessionPlaylists(list) => self.session = list,
                Event::Tracks { source_label, rows } => self.tracks = Some((source_label, rows)),
                Event::BusyStarted { label } => {
                    self.busy = Some(Busy {
                        label,
                        progress: None,
                    })
                }
                Event::BusyProgress { label, done, total } => {
                    self.busy = Some(Busy {
                        label,
                        progress: Some((done, total)),
                    });
                }
                Event::BusyFinished => self.busy = None,
                Event::Insights(data) => self.insights = Some(data),
                Event::GuardedDeleteArmed { id, name } => {
                    self.delete_dialog = Some(DeleteDialog {
                        id,
                        name,
                        input: String::new(),
                    });
                }
                Event::GuardedDeleteResolved => self.delete_dialog = None,
                Event::AiTest { ok, message } => self.ai_test = Some((ok, message)),
            }
        }
    }

    fn top_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Playlist Studio");
                ui.label(egui::RichText::new("for Spotify").weak());
                ui.separator();
                match &self.auth {
                    Some(a) if a.connected => {
                        ui.colored_label(
                            egui::Color32::from_rgb(30, 215, 96),
                            format!(
                                "● connected{}",
                                a.user
                                    .as_deref()
                                    .map(|u| format!(" as {u}"))
                                    .unwrap_or_default()
                            ),
                        );
                    }
                    _ => {
                        ui.colored_label(egui::Color32::GRAY, "○ not connected");
                        if ui.small_button("Connect…").clicked() {
                            self.send(Command::Connect);
                        }
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(busy) = &self.busy {
                        if let Some((done, total)) = busy.progress {
                            let fraction = total
                                .filter(|t| *t > 0)
                                .map(|t| (done as f32 / t as f32).clamp(0.0, 1.0));
                            let bar = match fraction {
                                Some(f) => egui::ProgressBar::new(f)
                                    .desired_width(140.0)
                                    .text(format!("{done}/{}", total.unwrap_or(0))),
                                None => egui::ProgressBar::new(0.0)
                                    .desired_width(140.0)
                                    .animate(true)
                                    .text(format!("{done}")),
                            };
                            ui.add(bar);
                        }
                        ui.spinner();
                        ui.label(egui::RichText::new(&busy.label).italics());
                    }
                });
            });
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("nav").default_size(190.0).show(ui, |ui| {
            ui.add_space(6.0);
            for (view, label) in [
                (View::Setup, "Setup Guide"),
                (View::Library, "Library"),
                (View::AiStudio, "AI Studio"),
                (View::Tools, "Tools"),
                (View::Insights, "Insights"),
                (View::Log, "Activity Log"),
                (View::Settings, "Settings"),
            ] {
                if ui.selectable_label(self.view == view, label).clicked() {
                    self.view = view;
                }
            }
            ui.separator();
            ui.label(egui::RichText::new("Session playlists").strong());
            ui.label(
                egui::RichText::new(
                    "Created this run — freely editable & deletable. They become protected \
                     when the app closes.",
                )
                .weak()
                .small(),
            );
            if self.session.is_empty() {
                ui.label(egui::RichText::new("(none yet)").weak());
            } else {
                for (_, name) in &self.session {
                    ui.label(format!("✏ {name}"));
                }
            }
            ui.separator();
            let refresh = ui.add_enabled(
                self.connected() && !self.is_busy(),
                egui::Button::new("⟳ Refresh playlists"),
            );
            if refresh.clicked() {
                self.send(Command::RefreshPlaylists);
            }
        });
    }

    fn delete_modal(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.delete_dialog else {
            return;
        };
        let mut action: Option<Command> = None;
        let modal = egui::Modal::new(egui::Id::new("guarded-delete")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(
                egui::RichText::new("⚠ Delete PROTECTED playlist")
                    .color(egui::Color32::from_rgb(230, 70, 70)),
            );
            ui.add_space(4.0);
            ui.label("You are about to permanently delete the protected playlist:");
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("“{}”", dialog.name))
                    .strong()
                    .size(18.0),
            );
            ui.add_space(6.0);
            ui.label(
                "This playlist was NOT created during this session. Playlist Studio never \
                 edits protected playlists, and deleting one is the only destructive action \
                 it allows — after this confirmation.",
            );
            ui.label(
                egui::RichText::new(
                    "Only this exact playlist will be affected. Owned playlists can be \
                     recovered for ~90 days via spotify.com/account → Recover playlists.",
                )
                .weak()
                .small(),
            );
            ui.add_space(8.0);
            ui.label(format!(
                "Type {DELETE_CONFIRMATION_WORD:?} (exactly, lowercase) to confirm. Anything \
                 else cancels."
            ));
            ui.text_edit_singleline(&mut dialog.input);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    action = Some(Command::CancelGuardedDelete);
                }
                let exact = dialog.input == DELETE_CONFIRMATION_WORD;
                let confirm_label = if exact {
                    egui::RichText::new("Delete this playlist").color(egui::Color32::WHITE)
                } else {
                    egui::RichText::new("Confirm (mismatch cancels)")
                };
                let mut button = egui::Button::new(confirm_label);
                if exact {
                    button = button.fill(egui::Color32::from_rgb(180, 40, 40));
                }
                if ui.add(button).clicked() {
                    action = Some(Command::ConfirmGuardedDelete {
                        id: dialog.id.clone(),
                        typed: dialog.input.clone(),
                    });
                }
            });
        });
        if modal.should_close() && action.is_none() {
            action = Some(Command::CancelGuardedDelete);
        }
        if let Some(cmd) = action {
            self.send(cmd);
            // Dialog closes on GuardedDeleteResolved from the worker.
        }
    }

    fn rename_modal(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.rename_dialog else {
            return;
        };
        let mut close = false;
        let mut submit: Option<Command> = None;
        egui::Window::new("Rename session playlist")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(format!("Current name: {}", dialog.old_name));
                ui.text_edit_singleline(&mut dialog.buffer);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    let valid = !dialog.buffer.trim().is_empty();
                    if ui.add_enabled(valid, egui::Button::new("Rename")).clicked() {
                        submit = Some(Command::RenameSession {
                            id: dialog.id.clone(),
                            current_name: dialog.old_name.clone(),
                            new_name: dialog.buffer.trim().to_string(),
                        });
                        close = true;
                    }
                });
            });
        if let Some(cmd) = submit {
            self.send(cmd);
        }
        if close {
            self.rename_dialog = None;
        }
    }
}

impl eframe::App for StudioApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.drain_events();
        let ctx = ui.ctx().clone();
        self.top_panel(ui);
        self.side_panel(ui);
        egui::CentralPanel::default().show(ui, |ui| match self.view {
            View::Setup => self.view_setup(ui),
            View::Library => self.view_library(ui),
            View::AiStudio => self.view_ai_studio(ui),
            View::Tools => self.view_tools(ui),
            View::Insights => self.view_insights(ui),
            View::Log => self.view_log(ui),
            View::Settings => self.view_settings(ui),
        });
        self.delete_modal(&ctx);
        self.rename_modal(&ctx);
    }
}
