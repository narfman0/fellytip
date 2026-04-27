//! Class selection screen shown on first join (before the player spawns).
//!
//! Displays an egui window with three class buttons (Warrior, Rogue, Mage).
//! When the player clicks a class the plugin sends a `ChooseClassMessage` via
//! Bevy's message system and hides itself.  The server plugin listens for that
//! message and spawns the player entity with the appropriate stats.
//!
//! The screen is only shown while `ClassSelectionState::open == true`.  It is
//! closed automatically once the local player entity appears in the world
//! (tag_local_player will insert `LocalPlayer`).

use bevy::ecs::message::MessageWriter;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use fellytip_shared::{
    combat::types::CharacterClass,
    protocol::ChooseClassMessage,
};

/// Resource tracking the class-selection overlay state.
#[derive(Resource)]
pub struct ClassSelectionState {
    /// Whether the window is currently visible.
    pub open: bool,
}

impl Default for ClassSelectionState {
    fn default() -> Self {
        Self { open: true }
    }
}

pub struct ClassSelectionPlugin;

impl Plugin for ClassSelectionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClassSelectionState>()
            .add_systems(Update, auto_close_on_spawn)
            .add_systems(EguiPrimaryContextPass, draw_class_selection);
    }
}

/// Close the class-selection screen once a player entity (with Experience) appears.
///
/// Detects spawn by checking for the `Experience` component — the same
/// heuristic used by `tag_local_player` in `main.rs`.
fn auto_close_on_spawn(
    local_player: Query<Entity, With<fellytip_shared::components::Experience>>,
    mut state: ResMut<ClassSelectionState>,
) {
    if state.open && !local_player.is_empty() {
        state.open = false;
    }
}

/// Render the class selection window.
fn draw_class_selection(
    mut ctx: EguiContexts,
    mut state: ResMut<ClassSelectionState>,
    mut writer: MessageWriter<ChooseClassMessage>,
) -> Result {
    if !state.open {
        return Ok(());
    }

    let egui_ctx = ctx.ctx_mut()?;

    egui::Window::new("Choose Your Class")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .resizable(false)
        .collapsible(false)
        .min_width(360.0)
        .show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Welcome to Fellytip");
                ui.label("Select a class to begin your adventure.");
                ui.add_space(16.0);
            });

            ui.separator();
            ui.add_space(8.0);

            // ── Warrior ──────────────────────────────────────────────────────
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(60, 30, 30))
                .inner_margin(egui::Margin::same(8))
                .corner_radius(egui::CornerRadius::same(4))
                .show(ui, |ui| {
                    ui.set_min_width(340.0);
                    ui.vertical(|ui| {
                        ui.colored_label(egui::Color32::from_rgb(240, 160, 80), "⚔  Warrior");
                        ui.label("Hit die: d10  |  Primary stat: Strength");
                        ui.label("Signature: Extra Attack — strike twice per action.");
                        ui.add_space(4.0);
                        if ui.button("  Play as Warrior  ").clicked() {
                            writer.write(ChooseClassMessage { class: CharacterClass::Warrior });
                            state.open = false;
                        }
                    });
                });

            ui.add_space(8.0);

            // ── Rogue ─────────────────────────────────────────────────────────
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(30, 50, 30))
                .inner_margin(egui::Margin::same(8))
                .corner_radius(egui::CornerRadius::same(4))
                .show(ui, |ui| {
                    ui.set_min_width(340.0);
                    ui.vertical(|ui| {
                        ui.colored_label(egui::Color32::from_rgb(100, 220, 120), "🗡  Rogue");
                        ui.label("Hit die: d8  |  Primary stat: Dexterity");
                        ui.label("Signature: Sneak Attack — bonus damage on hits.");
                        ui.add_space(4.0);
                        if ui.button("  Play as Rogue  ").clicked() {
                            writer.write(ChooseClassMessage { class: CharacterClass::Rogue });
                            state.open = false;
                        }
                    });
                });

            ui.add_space(8.0);

            // ── Mage ──────────────────────────────────────────────────────────
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(30, 30, 60))
                .inner_margin(egui::Margin::same(8))
                .corner_radius(egui::CornerRadius::same(4))
                .show(ui, |ui| {
                    ui.set_min_width(340.0);
                    ui.vertical(|ui| {
                        ui.colored_label(egui::Color32::from_rgb(120, 160, 255), "✦  Mage");
                        ui.label("Hit die: d6  |  Primary stat: Intellect");
                        ui.label("Signature: Arcane Surge — burst spell damage.");
                        ui.add_space(4.0);
                        if ui.button("  Play as Mage  ").clicked() {
                            writer.write(ChooseClassMessage { class: CharacterClass::Mage });
                            state.open = false;
                        }
                    });
                });

            ui.add_space(8.0);
        });

    Ok(())
}
