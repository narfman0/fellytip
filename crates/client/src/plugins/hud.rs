//! In-game HUD rendered via egui.
//!
//! Draws a compact stats panel anchored to the bottom-left corner of the screen
//! showing the local player's health bar and XP progress.  Only added in
//! windowed mode; headless builds skip this plugin entirely.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};
use fellytip_shared::components::{Experience, Health};
use lightyear::prelude::Replicated;

type LocalPlayerQuery<'w, 's> =
    Query<'w, 's, (&'static Health, &'static Experience), (With<Replicated>, With<Experience>)>;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .add_systems(EguiPrimaryContextPass, draw_hud);
    }
}

/// Draw the player stats overlay every frame.
fn draw_hud(
    mut ctx: EguiContexts,
    player_q: LocalPlayerQuery,
) -> Result {
    egui::Window::new("##stats")
        .anchor(egui::Align2::LEFT_BOTTOM, [10.0, -10.0])
        .resizable(false)
        .title_bar(false)
        .show(ctx.ctx_mut()?, |ui| {
            match player_q.single() {
                Ok((health, exp)) => {
                    let hp_frac =
                        (health.current as f32 / health.max.max(1) as f32).clamp(0.0, 1.0);
                    ui.label(format!("HP  {}/{}", health.current, health.max));
                    ui.add(
                        egui::ProgressBar::new(hp_frac)
                            .fill(egui::Color32::from_rgb(200, 50, 50)),
                    );
                    ui.label(format!("Lv {}   {}/{} XP", exp.level, exp.xp, exp.xp_to_next));
                    let xp_frac =
                        (exp.xp as f32 / exp.xp_to_next.max(1) as f32).clamp(0.0, 1.0);
                    ui.add(
                        egui::ProgressBar::new(xp_frac)
                            .fill(egui::Color32::from_rgb(50, 100, 200)),
                    );
                }
                Err(_) => {
                    ui.label("Connecting…");
                }
            }
        });
    Ok(())
}
