//! Playlist Studio — an AI-powered Spotify playlist manager with a strict
//! two-tier playlist-safety model. See README.md for the full story.

// Hide the console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod config;
mod messages;
mod ops;
mod safety;
mod shuffle;
mod spotify;
mod ui;
mod util;
mod worker;

use config::AppConfig;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "playlist_studio=info".into()),
        )
        .init();

    let cfg = AppConfig::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Playlist Studio")
            .with_inner_size([1300.0, 860.0])
            .with_min_inner_size([1000.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Playlist Studio",
        options,
        Box::new(|cc| Ok(Box::new(ui::StudioApp::new(cc, cfg)))),
    )
}
