//! In-game HUD rendered via egui.
//!
//! Panels:
//!   - Bottom-left: player health bar + XP progress.
//!   - Top-left: faction reputation standings.
//!   - Top-right: battle log.
//!   - Bottom-right: world story event feed.
//!
//! Only added in windowed mode; headless builds skip this plugin entirely.

use bevy::prelude::*;
use bevy_egui::{EguiContext, EguiContexts, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext, egui};
use fellytip_shared::{
    components::{Experience, Health, PlayerStandings},
    world::faction::standing_tier,
};
use crate::LocalPlayer;
use crate::plugins::battle::{BattleLog, ClientStoryLog};

type LocalPlayerQuery<'w, 's> =
    Query<'w, 's, (&'static Health, &'static Experience, Option<&'static PlayerStandings>), With<LocalPlayer>>;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .add_systems(PostStartup, ensure_primary_egui_context)
            .add_systems(EguiPrimaryContextPass, (draw_hud, draw_standings, draw_battle_log, draw_story_log));
    }
}

/// bevy_egui auto-setup occasionally misses the Camera3d on first frame; this guarantees it.
fn ensure_primary_egui_context(
    mut commands: Commands,
    cameras: Query<Entity, (With<Camera3d>, Without<EguiContext>)>,
) {
    for entity in &cameras {
        commands.entity(entity).insert(PrimaryEguiContext);
    }
}

/// Draw the player stats overlay every frame (bottom-left).
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
                Ok((health, exp, _)) => {
                    ui.heading(format!("Level {}", exp.level));
                    ui.separator();
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

/// Draw per-faction reputation standings (top-left).
fn draw_standings(
    mut ctx: EguiContexts,
    player_q: Query<&PlayerStandings, With<LocalPlayer>>,
) -> Result {
    let Ok(standings) = player_q.single() else { return Ok(()) };
    if standings.standings.is_empty() {
        return Ok(());
    }
    egui::Window::new("Faction Standing")
        .anchor(egui::Align2::LEFT_TOP, [10.0, 10.0])
        .resizable(false)
        .show(ctx.ctx_mut()?, |ui| {
            for (faction, score) in &standings.standings {
                let tier = standing_tier(*score);
                let color = tier_color(tier);
                ui.colored_label(color, format!("{faction}: {score:+} ({tier:?})"));
            }
        });
    Ok(())
}

fn tier_color(tier: fellytip_shared::world::faction::StandingTier) -> egui::Color32 {
    use fellytip_shared::world::faction::StandingTier;
    match tier {
        StandingTier::Exalted | StandingTier::Honored => egui::Color32::from_rgb(100, 220, 100),
        StandingTier::Friendly | StandingTier::Neutral => egui::Color32::from_rgb(200, 200, 200),
        StandingTier::Unfriendly => egui::Color32::from_rgb(230, 180, 80),
        StandingTier::Hostile | StandingTier::Hated   => egui::Color32::from_rgb(220, 60, 60),
    }
}

/// Draw the battle log panel (top-right).
fn draw_battle_log(
    mut ctx: EguiContexts,
    log: Res<BattleLog>,
) -> Result {
    egui::Window::new("Battle Log")
        .anchor(egui::Align2::LEFT_TOP, [10.0, 180.0])
        .resizable(false)
        .show(ctx.ctx_mut()?, |ui| {
            let entries = log.entries.iter().rev().take(20);
            for entry in entries {
                ui.label(entry.as_str());
            }
        });
    Ok(())
}

/// Draw the world story event feed (bottom-right).
fn draw_story_log(
    mut ctx: EguiContexts,
    log: Res<ClientStoryLog>,
) -> Result {
    if log.entries.is_empty() {
        return Ok(());
    }
    egui::Window::new("World Events")
        .anchor(egui::Align2::RIGHT_BOTTOM, [-10.0, -10.0])
        .resizable(false)
        .show(ctx.ctx_mut()?, |ui| {
            for entry in log.entries.iter().rev().take(10) {
                ui.label(entry.as_str());
            }
        });
    Ok(())
}
