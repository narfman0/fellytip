//! Sprite Studio — interactive egui frontend for sprite_gen.
//!
//! Usage:
//!   cargo run -p sprite_gen --bin sprite_studio

mod ui;

use anyhow::Result;
use eframe::egui::ViewportBuilder;

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("Sprite Studio")
            .with_inner_size([1280.0, 860.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Sprite Studio",
        options,
        Box::new(|cc| Ok(Box::new(ui::App::new(cc)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))?;

    Ok(())
}
