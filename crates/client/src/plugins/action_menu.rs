//! Right-click context menu for combat and interaction.
//!
//! The menu content adapts based on what was under the cursor when right-click
//! was pressed:
//!   - **Hostile entity**: Attack (targeted), Class Action, Dodge, Cancel
//!   - **No target / tile**: Attack (nearest), Ability, Dodge, Cancel

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use uuid::Uuid;
use fellytip_shared::bridge::LocalPlayerInput;
use fellytip_shared::inputs::ActionIntent;
use super::target_select::HoveredTarget;

/// Set to true this frame if egui consumed the pointer (so left-click attack is suppressed).
#[derive(Resource, Default)]
pub struct EguiPointerConsumed(pub bool);

/// What the cursor was over when the action menu was opened.
#[derive(Debug, Clone, Default)]
pub enum TargetContext {
    #[default]
    None,
    /// A specific hostile entity identified by its combat UUID.
    Hostile { uuid: Uuid },
}

#[derive(Resource, Default)]
pub struct ActionMenuState {
    pub open: bool,
    pub screen_pos: egui::Pos2,
    pub context: TargetContext,
}

pub struct ActionMenuPlugin;

impl Plugin for ActionMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActionMenuState>()
            .init_resource::<EguiPointerConsumed>()
            .add_systems(Update, handle_right_click)
            .add_systems(EguiPrimaryContextPass, draw_action_menu);
    }
}

fn handle_right_click(
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    windows: Query<&Window>,
    hovered: Option<Res<HoveredTarget>>,
    mut state: ResMut<ActionMenuState>,
) {
    let Some(mouse) = mouse else { return };
    if mouse.just_pressed(MouseButton::Right) {
        let cursor_pos = windows
            .single()
            .ok()
            .and_then(|w| w.cursor_position())
            .unwrap_or(Vec2::ZERO);
        state.open = true;
        state.screen_pos = egui::pos2(cursor_pos.x, cursor_pos.y);
        state.context = hovered
            .as_ref()
            .and_then(|h| h.0)
            .map(|(_, uuid)| TargetContext::Hostile { uuid })
            .unwrap_or(TargetContext::None);
    }
    if mouse.just_pressed(MouseButton::Left) && state.open {
        state.open = false;
    }
}

fn draw_action_menu(
    mut ctx: EguiContexts,
    mut state: ResMut<ActionMenuState>,
    mut local_input: ResMut<LocalPlayerInput>,
    mut consumed: ResMut<EguiPointerConsumed>,
) -> Result {
    consumed.0 = false;
    if !state.open {
        return Ok(());
    }

    let egui_ctx = ctx.ctx_mut()?;
    consumed.0 = egui_ctx.is_pointer_over_area();

    let context = state.context.clone();

    egui::Area::new("action_menu".into())
        .fixed_pos(state.screen_pos)
        .order(egui::Order::Foreground)
        .show(egui_ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(160.0);

                match &context {
                    TargetContext::Hostile { uuid } => {
                        let uuid = *uuid;
                        ui.label(egui::RichText::new("Combat").strong());
                        ui.separator();
                        if ui.button("⚔ Attack").clicked() {
                            local_input
                                .actions
                                .push((Some(ActionIntent::BasicAttack), Some(uuid)));
                            state.open = false;
                        }
                        if ui
                            .add_enabled(false, egui::Button::new("↗ Shove"))
                            .clicked()
                        {
                            state.open = false;
                        }
                        if ui
                            .add_enabled(false, egui::Button::new("🤝 Grapple"))
                            .clicked()
                        {
                            state.open = false;
                        }
                        ui.separator();
                        if ui.button("✦ Class Action").clicked() {
                            local_input
                                .actions
                                .push((Some(ActionIntent::UseAbility(1)), Some(uuid)));
                            state.open = false;
                        }
                        if ui.button("🛡 Dodge").clicked() {
                            local_input
                                .actions
                                .push((Some(ActionIntent::Dodge), None));
                            state.open = false;
                        }
                    }
                    TargetContext::None => {
                        ui.label(egui::RichText::new("Actions").strong());
                        ui.separator();
                        if ui.button("⚔ Attack").clicked() {
                            local_input
                                .actions
                                .push((Some(ActionIntent::BasicAttack), None));
                            state.open = false;
                        }
                        if ui.button("✦ Ability").clicked() {
                            local_input
                                .actions
                                .push((Some(ActionIntent::UseAbility(1)), None));
                            state.open = false;
                        }
                        if ui.button("🛡 Dodge").clicked() {
                            local_input
                                .actions
                                .push((Some(ActionIntent::Dodge), None));
                            state.open = false;
                        }
                        ui.separator();
                        if ui
                            .add_enabled(false, egui::Button::new("👁 Examine"))
                            .clicked()
                        {
                            state.open = false;
                        }
                    }
                }

                ui.separator();
                if ui.button("✕ Cancel").clicked() {
                    state.open = false;
                }
            });
        });

    Ok(())
}
