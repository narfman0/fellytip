//! sprite_studio — egui desktop app for browsing, generating, and approving
//! sprites for every entity in the bestiary.

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1024.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Sprite Studio",
        native_options,
        Box::new(|cc| Ok(Box::new(sprite_studio::studio::app::StudioApp::new(cc)))),
    )
}
