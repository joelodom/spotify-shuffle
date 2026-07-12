//! The background worker: a dedicated thread running a tokio runtime that
//! owns the `SpotifyService` (and with it the `SafetyPolicy`) plus the AI
//! provider. Commands are processed strictly sequentially, so a tier check
//! and the mutation it authorizes can never race.

use std::sync::mpsc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::ai::{AiProvider, AiRequest, build_provider};
use crate::config::{AppConfig, tokens_path};
use crate::messages::{AuthInfo, Command, Event, LogLevel, PlaylistRow, TrackRow};
use crate::ops::{self, OpUpdate};
use crate::safety::PlaylistId;
use crate::spotify::models::TrackInfo;
use crate::spotify::service::SpotifyService;
use crate::util::format_duration_ms;

#[derive(Clone)]
pub struct EventSender {
    tx: mpsc::Sender<Event>,
    ctx: egui::Context,
}

impl EventSender {
    pub fn send(&self, event: Event) {
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }
    pub fn log(&self, level: LogLevel, message: impl Into<String>) {
        let time = chrono::Local::now().format("%H:%M:%S").to_string();
        self.send(Event::Log(level, time, message.into()));
    }
}

pub fn spawn(
    cfg: AppConfig,
    ctx: egui::Context,
) -> (UnboundedSender<Command>, mpsc::Receiver<Event>) {
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (ev_tx, ev_rx) = mpsc::channel();
    let events = EventSender { tx: ev_tx, ctx };
    std::thread::Builder::new()
        .name("spotify-shuffle-worker".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(worker_loop(cfg, cmd_rx, events));
        })
        .expect("spawn worker thread");
    (cmd_tx, ev_rx)
}

struct Worker {
    cfg: AppConfig,
    svc: SpotifyService,
    ai: Option<Box<dyn AiProvider>>,
    ai_error: Option<String>,
    me_label: Option<String>,
    events: EventSender,
}

async fn worker_loop(cfg: AppConfig, mut rx: UnboundedReceiver<Command>, events: EventSender) {
    let mut worker = Worker::new(cfg, events);
    worker.emit_auth();
    if worker.svc.is_authenticated() {
        worker.events.log(
            LogLevel::Info,
            "Found saved Spotify login; connecting automatically…",
        );
        worker.busy("Loading playlists");
        worker.refresh_playlists().await;
        worker.done();
        if !worker.svc.is_authenticated() {
            // The saved refresh token was rejected (e.g. Spotify's 6-month
            // authorization expiry) and has been cleared — go straight back
            // through the browser approval rather than sitting disconnected.
            worker.events.log(
                LogLevel::Warn,
                "Saved login has expired — starting browser re-authorization…",
            );
            worker.connect().await;
        }
    } else if !worker.cfg.spotify.client_id.trim().is_empty() {
        // Credentials are configured but no login is saved yet: start the
        // one-time browser authorization automatically. (OAuth requires one
        // browser approval for user-data access even with a client secret;
        // every launch after that connects silently.)
        worker.events.log(
            LogLevel::Info,
            "Spotify credentials configured but no saved login — opening the one-time \
             browser authorization…",
        );
        worker.connect().await;
    }
    while let Some(cmd) = rx.recv().await {
        worker.handle(cmd).await;
    }
}

impl Worker {
    fn new(cfg: AppConfig, events: EventSender) -> Self {
        let svc = SpotifyService::new(
            &cfg.spotify.client_id,
            cfg.spotify.client_secret_opt(),
            tokens_path(),
        );
        let (ai, ai_error) = match build_provider(&cfg.ai) {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e.to_string())),
        };
        if let Some(err) = &ai_error {
            events.log(LogLevel::Warn, format!("AI provider unavailable: {err}"));
        }
        Self {
            cfg,
            svc,
            ai,
            ai_error,
            me_label: None,
            events,
        }
    }

    fn provider_desc(&self) -> String {
        match (&self.ai, &self.ai_error) {
            (Some(p), _) => p.describe(),
            (None, Some(e)) => format!("unavailable — {e}"),
            (None, None) => "not configured".into(),
        }
    }

    fn auth_method(&self) -> String {
        if self.cfg.spotify.client_secret_opt().is_some() {
            "client secret (confidential-client flow)".into()
        } else {
            "PKCE (no client secret)".into()
        }
    }

    fn emit_auth(&self) {
        self.events.send(Event::Auth(AuthInfo {
            connected: self.svc.is_authenticated(),
            user: self.me_label.clone(),
            auth_method: self.auth_method(),
            provider_desc: self.provider_desc(),
        }));
    }

    fn busy(&self, label: &str) {
        self.events.send(Event::BusyStarted {
            label: label.to_string(),
        });
    }
    fn done(&self) {
        self.events.send(Event::BusyFinished);
    }

    fn ai(&self) -> Result<&dyn AiProvider, String> {
        match &self.ai {
            Some(p) => Ok(&**p),
            None => Err(format!(
                "AI provider not available ({}) — check Settings",
                self.ai_error.as_deref().unwrap_or("not configured")
            )),
        }
    }

    async fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Connect => self.connect().await,
            Command::Disconnect => {
                self.svc.disconnect();
                self.me_label = None;
                self.events
                    .log(LogLevel::Info, "Disconnected from Spotify (tokens deleted)");
                self.emit_auth();
                self.events.send(Event::Playlists(Vec::new()));
                self.events.send(Event::SessionPlaylists(Vec::new()));
            }
            Command::RefreshPlaylists => {
                self.busy("Loading playlists");
                self.refresh_playlists().await;
                self.done();
            }
            Command::LoadTracks(source) => {
                self.busy(&format!("Loading '{}'", source.label()));
                let events = self.events.clone();
                let mut sink = sink_of(&events);
                match ops::fetch_source_tracks(&mut self.svc, &source, &mut sink).await {
                    Ok(tracks) => {
                        let rows = tracks.iter().map(track_row).collect();
                        self.events.send(Event::Tracks {
                            source_label: source.label(),
                            rows,
                        });
                    }
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Load failed: {e}")),
                }
                self.done();
            }

            Command::Generate {
                description,
                count,
                personalize,
            } => {
                if let Err(e) = self.ai() {
                    return self.events.log(LogLevel::Error, e);
                }
                self.busy("AI playlist creation");
                let events = self.events.clone();
                let mut sink = sink_of(&events);
                let public = self.cfg.spotify.create_public;
                let ai = self.ai.as_deref().expect("checked above");
                match ops::generate::generate_playlist(
                    &mut self.svc,
                    ai,
                    &description,
                    count,
                    personalize,
                    public,
                    &mut sink,
                )
                .await
                {
                    Ok(out) => {
                        self.events.log(
                            LogLevel::Success,
                            format!(
                                "Created '{}' with {} tracks ({} matched, {} unmatched)",
                                out.playlist.name,
                                out.playlist.track_count,
                                out.report.resolved,
                                out.report.unresolved.len()
                            ),
                        );
                        self.refresh_playlists().await;
                    }
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Generation failed: {e}")),
                }
                self.done();
            }
            Command::Refine {
                source,
                instruction,
                in_place,
            } => {
                if let Err(e) = self.ai() {
                    return self.events.log(LogLevel::Error, e);
                }
                self.busy(&format!("AI refinement of '{}'", source.label()));
                let events = self.events.clone();
                let mut sink = sink_of(&events);
                let public = self.cfg.spotify.create_public;
                let ai = self.ai.as_deref().expect("checked above");
                match ops::refine::refine_playlist(
                    &mut self.svc,
                    ai,
                    &source,
                    &instruction,
                    in_place,
                    public,
                    &mut sink,
                )
                .await
                {
                    Ok(out) => {
                        self.events.log(
                            LogLevel::Success,
                            format!(
                                "Refinement done — {} ({} matched, {} unmatched)",
                                out.destination.summary(),
                                out.report.resolved,
                                out.report.unresolved.len()
                            ),
                        );
                        self.refresh_playlists().await;
                    }
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Refinement failed: {e}")),
                }
                self.done();
            }
            Command::Organize {
                goal,
                max_playlists,
            } => {
                if let Err(e) = self.ai() {
                    return self.events.log(LogLevel::Error, e);
                }
                self.busy("Learning from your library");
                let events = self.events.clone();
                let mut sink = sink_of(&events);
                let public = self.cfg.spotify.create_public;
                let ai = self.ai.as_deref().expect("checked above");
                match ops::organize::organize_library(
                    &mut self.svc,
                    ai,
                    &goal,
                    max_playlists,
                    public,
                    &mut sink,
                )
                .await
                {
                    Ok(out) => {
                        let names: Vec<String> =
                            out.created.iter().map(|p| p.name.clone()).collect();
                        let unmatched = if out.unresolved_total > 0 {
                            format!(
                                " ({} suggestion(s) had no Spotify match)",
                                out.unresolved_total
                            )
                        } else {
                            String::new()
                        };
                        self.events.log(
                            LogLevel::Success,
                            format!(
                                "Created {} playlist(s): {}{unmatched}{}",
                                out.created.len(),
                                names.join(", "),
                                out.notes.map(|n| format!(" — {n}")).unwrap_or_default()
                            ),
                        );
                        self.refresh_playlists().await;
                    }
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Organize failed: {e}")),
                }
                self.done();
            }

            Command::Shuffle { source } => {
                self.busy(&format!("Unbiased shuffle of '{}'", source.label()));
                let events = self.events.clone();
                let mut sink = sink_of(&events);
                let public = self.cfg.spotify.create_public;
                match ops::playlist_tools::shuffle_to_new_playlist(
                    &mut self.svc,
                    &source,
                    public,
                    &mut sink,
                )
                .await
                {
                    Ok(dest) => {
                        self.events.log(
                            LogLevel::Success,
                            format!("Shuffle done — {}", dest.summary()),
                        );
                        self.refresh_playlists().await;
                    }
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Shuffle failed: {e}")),
                }
                self.done();
            }
            Command::Dedupe { source, in_place } => {
                self.busy(&format!("Deduplicating '{}'", source.label()));
                let events = self.events.clone();
                let mut sink = sink_of(&events);
                let public = self.cfg.spotify.create_public;
                match ops::playlist_tools::dedupe_playlist(
                    &mut self.svc,
                    &source,
                    in_place,
                    public,
                    &mut sink,
                )
                .await
                {
                    Ok(out) => match out.destination {
                        Some(dest) => {
                            self.events.log(
                                LogLevel::Success,
                                format!(
                                    "Removed {} duplicate(s) — {}",
                                    out.removed.len(),
                                    dest.summary()
                                ),
                            );
                            self.refresh_playlists().await;
                        }
                        None => self
                            .events
                            .log(LogLevel::Info, "No duplicates found; nothing was created"),
                    },
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Dedupe failed: {e}")),
                }
                self.done();
            }
            Command::Merge {
                sources,
                dedupe,
                shuffle,
            } => {
                self.busy("Merging sources");
                let events = self.events.clone();
                let mut sink = sink_of(&events);
                let public = self.cfg.spotify.create_public;
                match ops::playlist_tools::merge_playlists(
                    &mut self.svc,
                    &sources,
                    dedupe,
                    shuffle,
                    public,
                    &mut sink,
                )
                .await
                {
                    Ok(dest) => {
                        self.events.log(
                            LogLevel::Success,
                            format!("Merge done — {}", dest.summary()),
                        );
                        self.refresh_playlists().await;
                    }
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Merge failed: {e}")),
                }
                self.done();
            }
            Command::Sort {
                source,
                key,
                descending,
                in_place,
            } => {
                self.busy(&format!("Sorting '{}'", source.label()));
                let events = self.events.clone();
                let mut sink = sink_of(&events);
                let public = self.cfg.spotify.create_public;
                match ops::playlist_tools::sort_playlist(
                    &mut self.svc,
                    &source,
                    key,
                    descending,
                    in_place,
                    public,
                    &mut sink,
                )
                .await
                {
                    Ok(dest) => {
                        self.events
                            .log(LogLevel::Success, format!("Sort done — {}", dest.summary()));
                        self.refresh_playlists().await;
                    }
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Sort failed: {e}")),
                }
                self.done();
            }
            Command::Import { name, text } => {
                self.busy("Importing track list");
                let events = self.events.clone();
                let mut sink = sink_of(&events);
                let public = self.cfg.spotify.create_public;
                match ops::import_export::import_tracks(
                    &mut self.svc,
                    &name,
                    &text,
                    public,
                    &mut sink,
                )
                .await
                {
                    Ok(out) => {
                        self.events.log(
                            LogLevel::Success,
                            format!(
                                "Import done — {} ({} matched, {} unmatched, {} unparsable line(s))",
                                out.destination.summary(),
                                out.report.resolved,
                                out.report.unresolved.len(),
                                out.skipped_lines
                            ),
                        );
                        self.refresh_playlists().await;
                    }
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Import failed: {e}")),
                }
                self.done();
            }
            Command::Export {
                source,
                format,
                path,
            } => {
                self.busy(&format!("Exporting '{}'", source.label()));
                let events = self.events.clone();
                let mut sink = sink_of(&events);
                match ops::import_export::export_source(
                    &mut self.svc,
                    &source,
                    format,
                    &path,
                    &mut sink,
                )
                .await
                {
                    Ok(summary) => self.events.log(LogLevel::Success, summary),
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Export failed: {e}")),
                }
                self.done();
            }

            Command::RenameSession {
                id,
                current_name,
                new_name,
            } => {
                match self
                    .svc
                    .rename_playlist(&id, &current_name, Some(&new_name), None)
                    .await
                {
                    Ok(()) => {
                        self.events.log(
                            LogLevel::Success,
                            format!("Renamed '{current_name}' to '{new_name}'"),
                        );
                        self.refresh_playlists().await;
                    }
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Rename failed: {e}")),
                }
            }
            Command::DeleteSession { id, name } => {
                match self.svc.delete_session_playlist(&id, &name).await {
                    Ok(deleted) => {
                        self.events.log(
                            LogLevel::Success,
                            format!(
                                "Deleted session playlist '{deleted}' (no confirmation needed)"
                            ),
                        );
                        self.refresh_playlists().await;
                    }
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Delete failed: {e}")),
                }
            }
            Command::ArmGuardedDelete { id, name } => {
                let pending = self.svc.begin_guarded_delete(id, &name);
                self.events.log(
                    LogLevel::Warn,
                    format!(
                        "Guarded deletion armed for protected playlist '{}' — type \"delete\" \
                         in the dialog to proceed",
                        pending.name
                    ),
                );
                self.events.send(Event::GuardedDeleteArmed {
                    id: pending.id,
                    name: pending.name,
                });
            }
            Command::ConfirmGuardedDelete { id, typed } => {
                match self.svc.confirm_guarded_delete(&id, &typed).await {
                    Ok(name) => {
                        self.events.log(
                            LogLevel::Success,
                            format!(
                                "Deleted protected playlist '{name}'. (Owned playlists can be \
                                 restored for ~90 days at spotify.com/account → Recover playlists.)"
                            ),
                        );
                        self.refresh_playlists().await;
                    }
                    Err(e) => self
                        .events
                        .log(LogLevel::Warn, format!("Deletion NOT performed: {e}")),
                }
                self.events.send(Event::GuardedDeleteResolved);
            }
            Command::CancelGuardedDelete => {
                if self.svc.cancel_guarded_delete() {
                    self.events
                        .log(LogLevel::Info, "Guarded deletion cancelled");
                }
                self.events.send(Event::GuardedDeleteResolved);
            }

            Command::FetchInsights => {
                self.busy("Gathering listening insights");
                let events = self.events.clone();
                let mut sink = sink_of(&events);
                match ops::insights::gather_insights(&mut self.svc, &mut sink).await {
                    Ok(data) => self.events.send(Event::Insights(Box::new(data))),
                    Err(e) => self
                        .events
                        .log(LogLevel::Error, format!("Insights failed: {e}")),
                }
                self.done();
            }
            Command::CheckAi => {
                self.busy("Checking AI provider");
                let result = match self.ai() {
                    Err(e) => Event::AiTest {
                        ok: false,
                        message: e,
                    },
                    Ok(ai) => match ai.health_check().await {
                        Ok(health) => Event::AiTest {
                            ok: true,
                            message: health,
                        },
                        Err(e) => Event::AiTest {
                            ok: false,
                            message: e.to_string(),
                        },
                    },
                };
                self.events.send(result);
                self.done();
            }
            Command::TestAi => {
                self.busy("Testing AI provider");
                let result = match self.ai() {
                    Err(e) => Event::AiTest {
                        ok: false,
                        message: e,
                    },
                    Ok(ai) => match ai.health_check().await {
                        Err(e) => Event::AiTest {
                            ok: false,
                            message: e.to_string(),
                        },
                        Ok(health) => {
                            self.events.log(LogLevel::Info, format!("Health: {health}"));
                            let req = AiRequest {
                                system: String::new(),
                                user: "Reply with exactly: OK".into(),
                            };
                            match ai.complete(&req).await {
                                Ok(text) => Event::AiTest {
                                    ok: true,
                                    message: format!(
                                        "{health} — test generation returned: {}",
                                        text.trim().chars().take(80).collect::<String>()
                                    ),
                                },
                                Err(e) => Event::AiTest {
                                    ok: false,
                                    message: e.to_string(),
                                },
                            }
                        }
                    },
                };
                self.events.send(result);
                self.done();
            }
            Command::ApplyConfig(new_cfg) => {
                let new_cfg = *new_cfg;
                let client_changed = new_cfg.spotify.client_id != self.cfg.spotify.client_id
                    || new_cfg.spotify.client_secret != self.cfg.spotify.client_secret;
                if let Err(e) = new_cfg.save() {
                    self.events
                        .log(LogLevel::Error, format!("Could not save config: {e}"));
                }
                self.cfg = new_cfg;
                if client_changed {
                    self.svc.reconfigure(
                        &self.cfg.spotify.client_id,
                        self.cfg.spotify.client_secret_opt(),
                        tokens_path(),
                    );
                    self.me_label = None;
                    self.events.log(
                        LogLevel::Info,
                        "Spotify credentials changed — reconnect from Settings",
                    );
                }
                let (ai, ai_error) = match build_provider(&self.cfg.ai) {
                    Ok(p) => (Some(p), None),
                    Err(e) => (None, Some(e.to_string())),
                };
                self.ai = ai;
                self.ai_error = ai_error;
                self.events.log(
                    LogLevel::Success,
                    format!(
                        "Settings applied — Spotify auth method: {}",
                        self.auth_method()
                    ),
                );
                self.emit_auth();
            }
        }
    }

    async fn connect(&mut self) {
        if self.cfg.spotify.client_id.is_empty() {
            self.events.log(
                LogLevel::Error,
                "Set your Spotify app Client ID in Settings first (see README for the \
                 dashboard walkthrough)",
            );
            return;
        }
        self.busy("Connecting to Spotify");
        let events = self.events.clone();
        let client_id = self.cfg.spotify.client_id.clone();
        let client_secret = self.cfg.spotify.client_secret_opt().map(str::to_string);
        let port = self.cfg.spotify.redirect_port;
        let result = self
            .svc
            .connect_interactive(&client_id, client_secret.as_deref(), port, move |line| {
                events.log(LogLevel::Info, line);
            })
            .await;
        match result {
            Ok(user) => {
                self.me_label = Some(user.label().to_string());
                self.events.log(
                    LogLevel::Success,
                    format!("Connected to Spotify as {}", user.label()),
                );
                self.emit_auth();
                self.refresh_playlists().await;
            }
            Err(e) => {
                self.events
                    .log(LogLevel::Error, format!("Spotify connection failed: {e}"));
                self.emit_auth();
            }
        }
        self.done();
    }

    async fn refresh_playlists(&mut self) {
        let me_id = match self.svc.ensure_me().await {
            Ok(me) => {
                self.me_label = Some(me.label().to_string());
                Some(me.id)
            }
            Err(e) => {
                self.events
                    .log(LogLevel::Error, format!("Could not load profile: {e}"));
                self.emit_auth();
                return;
            }
        };
        self.emit_auth();
        let events = self.events.clone();
        let mut progress = |done: u64, total: Option<u64>| {
            events.send(Event::BusyProgress {
                label: "Loading playlists".into(),
                done,
                total,
            });
        };
        match self.svc.reads().my_playlists(&mut progress).await {
            Ok(list) => {
                let rows: Vec<PlaylistRow> = list
                    .iter()
                    .map(|p| {
                        let id = PlaylistId(p.id.clone());
                        let owner_id = p.owner.as_ref().map(|o| o.id.as_str());
                        PlaylistRow {
                            tier: self.svc.tier(&id),
                            id,
                            name: p.name.clone(),
                            owner: p.owner_label(),
                            total: p.total_tracks(),
                            readable: p.collaborative || owner_id == me_id.as_deref(),
                        }
                    })
                    .collect();
                self.events
                    .log(LogLevel::Info, format!("Loaded {} playlists", rows.len()));
                self.events.send(Event::Playlists(rows));
                self.events
                    .send(Event::SessionPlaylists(self.svc.session_playlists()));
            }
            Err(e) => self
                .events
                .log(LogLevel::Error, format!("Could not list playlists: {e}")),
        }
    }
}

fn sink_of(events: &EventSender) -> impl FnMut(OpUpdate) + '_ {
    move |update: OpUpdate| match update {
        OpUpdate::Log(message) => events.log(LogLevel::Info, message),
        OpUpdate::Progress { label, done, total } => {
            events.send(Event::BusyProgress { label, done, total })
        }
    }
}

fn track_row(t: &TrackInfo) -> TrackRow {
    TrackRow {
        title: t.name.clone(),
        artists: t.artist_line(),
        album: t.album.clone(),
        duration: format_duration_ms(t.duration_ms),
        release_date: t.release_date.clone().unwrap_or_default(),
        added_at: t
            .added_at
            .as_deref()
            .map(|s| s.chars().take(10).collect())
            .unwrap_or_default(),
        is_local: t.is_local,
        is_episode: t.is_episode,
    }
}
