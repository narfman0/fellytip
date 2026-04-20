use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

#[derive(Resource, Default)]
pub struct PauseMenu {
    pub open: bool,
}

pub struct PauseMenuPlugin;

impl Plugin for PauseMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PauseMenu>()
            .add_systems(Update, toggle_pause_menu)
            .add_systems(EguiPrimaryContextPass, draw_pause_menu);
    }
}

fn toggle_pause_menu(keyboard: Res<ButtonInput<KeyCode>>, mut menu: ResMut<PauseMenu>) {
    if keyboard.just_pressed(KeyCode::Escape) {
        menu.open = !menu.open;
    }
}

fn draw_pause_menu(
    mut ctx: EguiContexts,
    mut menu: ResMut<PauseMenu>,
) -> Result {
    let egui_ctx = ctx.ctx_mut()?;

    if !menu.open {
        return Ok(());
    }

    egui::Window::new("Paused")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .resizable(false)
        .collapsible(false)
        .show(egui_ctx, |ui| {
            ui.set_min_width(200.0);
            ui.vertical_centered(|ui| {
                if ui.button("New Game").clicked() {
                    // MULTIPLAYER: disconnect from server here.
                    menu.open = false;
                }
                ui.add_space(8.0);
                if ui.button("Exit Game").clicked() {
                    std::process::exit(0);
                }
            });
        });
    Ok(())
}
