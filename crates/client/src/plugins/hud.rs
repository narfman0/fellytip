//! In-game HUD rendered via egui.
//!
//! Panels:
//!   - Bottom-left: player health bar + XP progress.
//!   - Top-left: battle log.
//!   - Bottom-right: world story event feed.
//!   - 'C' key: character screen overlay with detailed stats and faction standings.
//!
//! Only added in windowed mode; headless builds skip this plugin entirely.

use bevy::prelude::*;
use bevy_egui::{EguiContext, EguiContexts, EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext, egui};
use fellytip_shared::{
    components::{ActionBudget, Experience, Health, PlayerStandings},
    world::faction::standing_tier,
};
use fellytip_server::plugins::party::PartyRegistry;
use crate::LocalPlayer;
use crate::plugins::battle::{BattleLog, ClientStoryLog};
use crate::plugins::debug_console::DebugConsole;
use crate::plugins::pause_menu::PauseMenu;

type LocalPlayerQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Health,
        &'static Experience,
        Option<&'static PlayerStandings>,
        Option<&'static ActionBudget>,
    ),
    With<LocalPlayer>,
>;

/// Resource tracking whether the character screen is open.
#[derive(Resource, Default)]
pub struct CharScreen {
    pub open: bool,
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<CharScreen>()
            // Disable bevy_egui auto-detection and explicitly tag the Camera3d
            // as the primary context in PostStartup (after spawn_camera runs in
            // Startup).  This prevents duplicate PrimaryEguiContext components
            // when Bevy internals spawn additional bare Camera entities.
            .add_systems(PreStartup, disable_egui_auto_context)
            .add_systems(PostStartup, tag_primary_egui_camera)
            .add_systems(Update, toggle_char_screen)
            .add_systems(EguiPrimaryContextPass, (draw_hud, draw_party_hud, draw_char_screen, draw_battle_log, draw_story_log));
    }
}

/// Turn off bevy_egui's automatic primary-context creation so we can
/// assign it explicitly to the correct Camera3d entity.
fn disable_egui_auto_context(mut settings: ResMut<EguiGlobalSettings>) {
    settings.auto_create_primary_context = false;
}

/// Tag the orbit Camera3d as the sole primary egui context.
fn tag_primary_egui_camera(
    mut commands: Commands,
    cameras: Query<Entity, With<Camera3d>>,
) {
    if let Ok(entity) = cameras.single() {
        commands.entity(entity).insert((EguiContext::default(), PrimaryEguiContext));
    }
}

fn toggle_char_screen(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut char_screen: ResMut<CharScreen>,
    console: Option<Res<DebugConsole>>,
    pause_menu: Option<Res<PauseMenu>>,
) {
    if console.is_some_and(|c| c.open) || pause_menu.is_some_and(|m| m.open) {
        return;
    }
    if keyboard.just_pressed(KeyCode::KeyC) {
        char_screen.open = !char_screen.open;
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
                Ok((health, exp, _, budget_opt)) => {
                    ui.heading(format!("Level {}", exp.level));
                    ui.separator();
                    let hp_frac =
                        (health.current as f32 / health.max.max(1) as f32).clamp(0.0, 1.0);
                    ui.label(format!("HP  {}/{}", health.current, health.max));
                    ui.add(
                        egui::ProgressBar::new(hp_frac)
                            .fill(egui::Color32::from_rgb(200, 50, 50)),
                    );
                    let xp_frac =
                        (exp.xp as f32 / exp.xp_to_next.max(1) as f32).clamp(0.0, 1.0);
                    ui.label(format!("XP  {}/{}", exp.xp, exp.xp_to_next));
                    ui.add(
                        egui::ProgressBar::new(xp_frac)
                            .fill(egui::Color32::from_rgb(50, 100, 200)),
                    );
                    if let Some(budget) = budget_opt {
                        ui.separator();
                        ui.horizontal(|ui| {
                            let ready   = egui::Color32::from_rgb(80, 180, 255);
                            let spent   = egui::Color32::from_rgb(60, 60, 60);
                            let pip = |avail: bool| if avail { ready } else { spent };
                            ui.colored_label(pip(budget.action),       "● A");
                            ui.colored_label(pip(budget.bonus_action), "● B");
                            ui.colored_label(pip(budget.reaction),     "◆ R");
                        });
                    }
                }
                Err(_) => {
                    ui.label("Connecting…");
                }
            }
        });
    Ok(())
}

/// Draw the party members panel (top-right).
///
/// Shows each party member's name and a health bar (current HP / max HP).
/// Only visible when the local player is in a party with at least one other member.
fn draw_party_hud(
    mut ctx: EguiContexts,
    party_registry: Option<Res<PartyRegistry>>,
    local_q: Query<Entity, With<LocalPlayer>>,
    health_q: Query<(&Health, Option<&Experience>)>,
) -> Result {
    let Some(registry) = party_registry else { return Ok(()) };

    // Find which party (if any) the local player belongs to.
    let Ok(local_entity) = local_q.single() else { return Ok(()) };

    let party = registry.parties.iter().find(|p| p.members.contains(&local_entity));
    let Some(party) = party else { return Ok(()) };

    // Only render the panel when there are other party members to show.
    let others: Vec<Entity> = party.members.iter()
        .copied()
        .filter(|&e| e != local_entity)
        .collect();
    if others.is_empty() {
        return Ok(());
    }

    egui::Window::new("Party")
        .anchor(egui::Align2::RIGHT_TOP, [-10.0, 10.0])
        .resizable(false)
        .title_bar(true)
        .show(ctx.ctx_mut()?, |ui| {
            for (slot, &entity) in others.iter().enumerate() {
                match health_q.get(entity) {
                    Ok((health, exp)) => {
                        let level = exp.map(|e| e.level).unwrap_or(1);
                        ui.label(format!("Member {} (Lv {})", slot + 1, level));
                        let hp_frac =
                            (health.current as f32 / health.max.max(1) as f32).clamp(0.0, 1.0);
                        ui.label(format!("{}/{}", health.current, health.max));
                        ui.add(
                            egui::ProgressBar::new(hp_frac)
                                .fill(egui::Color32::from_rgb(200, 50, 50)),
                        );
                        if slot + 1 < others.len() {
                            ui.separator();
                        }
                    }
                    Err(_) => {
                        ui.label(format!("Member {} — unknown", slot + 1));
                    }
                }
            }
        });
    Ok(())
}

/// Draw the character screen overlay when 'C' is pressed.
fn draw_char_screen(
    mut ctx: EguiContexts,
    player_q: LocalPlayerQuery,
    mut char_screen: ResMut<CharScreen>,
) -> Result {
    if !char_screen.open {
        return Ok(());
    }
    let mut open = char_screen.open;
    egui::Window::new("Character")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .resizable(false)
        .open(&mut open)
        .show(ctx.ctx_mut()?, |ui| {
            match player_q.single() {
                Ok((health, exp, standings_opt, _)) => {
                    ui.heading(format!("Level {}", exp.level));
                    ui.separator();

                    egui::Grid::new("char_stats").num_columns(2).spacing([20.0, 4.0]).show(ui, |ui| {
                        ui.label("Health:");
                        ui.label(format!("{}/{}", health.current, health.max));
                        ui.end_row();
                        ui.label("Experience:");
                        ui.label(format!("{}/{}", exp.xp, exp.xp_to_next));
                        ui.end_row();
                    });

                    ui.separator();
                    ui.heading("Faction Standing");
                    ui.separator();

                    if let Some(standings) = standings_opt {
                        if standings.standings.is_empty() {
                            ui.label("No known factions.");
                        } else {
                            for (faction, score) in &standings.standings {
                                let tier = standing_tier(*score);
                                let color = tier_color(tier);
                                ui.colored_label(color, format!("{faction}: {score:+} ({tier:?})"));
                            }
                        }
                    } else {
                        ui.label("No faction data.");
                    }
                }
                Err(_) => {
                    ui.label("Connecting…");
                }
            }
        });
    char_screen.open = open;
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

/// Draw the battle log panel (top-left).
fn draw_battle_log(
    mut ctx: EguiContexts,
    log: Res<BattleLog>,
) -> Result {
    egui::Window::new("Battle Log")
        .anchor(egui::Align2::LEFT_TOP, [10.0, 10.0])
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
