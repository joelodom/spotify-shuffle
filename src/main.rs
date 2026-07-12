//! Spotify Shuffle — an AI-powered Spotify playlist manager with a strict
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
                .unwrap_or_else(|_| "spotify_shuffle=info".into()),
        )
        .init();

    let cfg = AppConfig::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Spotify Shuffle")
            .with_inner_size([1420.0, 920.0])
            .with_min_inner_size([1120.0, 780.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Spotify Shuffle",
        options,
        Box::new(|cc| Ok(Box::new(ui::StudioApp::new(cc, cfg)))),
    )
}
