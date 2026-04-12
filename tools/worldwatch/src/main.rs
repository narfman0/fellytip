//! Fellytip WorldWatch — Windows tray app for live server monitoring.
//!
//! # Architecture
//!
//! - A tokio multi-thread runtime runs a background BRP+SQLite polling task.
//! - eframe blocks the main thread running the egui window.
//! - The two sides share an `Arc<Mutex<WorldSnapshot>>` updated every 2 s.
//! - Tray icon events are polled inside eframe's `update()` loop via
//!   `TrayIconEvent::receiver().try_recv()` — no extra thread required.
//!
//! # Usage
//!
//! ```
//! cargo run -p worldwatch
//! ```
//!
//! DB path resolution (first match wins):
//!   1. `WORLDWATCH_DB` environment variable
//!   2. `./fellytip.db`  (both tools launched from workspace root)

mod app;
mod brp;
mod db;
mod state;

use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use anyhow::Result;
use eframe::egui::ViewportBuilder;
use tray_icon::{Icon, TrayIconBuilder};
use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};

use app::WorldWatchApp;
use state::WorldSnapshot;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Resolve DB path.
    let db_path = std::env::var("WORLDWATCH_DB")
        .unwrap_or_else(|_| "fellytip.db".to_owned());
    if !std::path::Path::new(&db_path).exists() {
        tracing::warn!(path = %db_path, "DB file not found — story/faction/ecology tabs will be empty");
    }

    // ── Tokio runtime ─────────────────────────────────────────────────────────
    // Must be built before eframe::run_native, which blocks the main thread.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;

    let snapshot: Arc<Mutex<WorldSnapshot>> = Arc::new(Mutex::new(WorldSnapshot::default()));

    // Channel pair for freeform BRP queries from the Query tab.
    let (query_tx, query_rx) = mpsc::channel::<String>();
    let (result_tx, result_rx) = mpsc::channel::<String>();

    {
        let snapshot = Arc::clone(&snapshot);
        let db_path = db_path.clone();
        rt.spawn(async move {
            state::polling_loop(snapshot, query_rx, result_tx, db_path).await;
        });
    }

    // ── Tray icon ─────────────────────────────────────────────────────────────
    let icon = load_icon()?;

    let show_hide = MenuItem::new("Show / Hide", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let show_hide_id = show_hide.id().clone();
    let quit_id = quit.id().clone();

    let menu = Menu::new();
    menu.append(&show_hide)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit)?;

    let tray = TrayIconBuilder::new()
        .with_tooltip("Fellytip WorldWatch")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .build()?;

    // ── eframe ────────────────────────────────────────────────────────────────
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("Fellytip WorldWatch")
            .with_inner_size([960.0, 640.0])
            .with_visible(false), // start hidden; tray click shows it
        ..Default::default()
    };

    let app = WorldWatchApp::new(
        Arc::clone(&snapshot),
        tray,
        query_tx,
        result_rx,
        show_hide_id,
        quit_id,
    );

    // eframe::Error is not Send+Sync so it can't use `?` with anyhow::Result.
    eframe::run_native(
        "WorldWatch",
        options,
        Box::new(|_cc| Ok(Box::new(app))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;

    // Tokio runtime is kept alive until eframe exits, then shut down cleanly.
    rt.shutdown_timeout(Duration::from_secs(2));
    Ok(())
}

/// Load the tray icon from embedded PNG bytes.
fn load_icon() -> Result<Icon> {
    // Embedded at compile time — always available even after installation.
    let png_bytes = include_bytes!("../assets/tray_icon.png");
    let img = image::load_from_memory(png_bytes)?.into_rgba8();
    let (width, height) = img.dimensions();
    let rgba = img.into_raw();
    Ok(Icon::from_rgba(rgba, width, height)?)
}
