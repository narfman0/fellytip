//! Right-click action popup menu for combat.

use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use fellytip_shared::inputs::ActionIntent;
use fellytip_server::plugins::combat::LocalPlayerInput;

/// Set to true this frame if egui consumed the pointer (so left-click attack is suppressed).
#[derive(Resource, Default)]
pub struct EguiPointerConsumed(pub bool);

#[derive(Resource, Default)]
pub struct ActionMenuState {
    pub open: bool,
    pub screen_pos: egui::Pos2,
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
    mut state: ResMut<ActionMenuState>,
) {
    let Some(mouse) = mouse else { return };
    if mouse.just_pressed(MouseButton::Right) {
        let cursor_pos = windows.single()
            .ok()
            .and_then(|w| w.cursor_position())
            .unwrap_or(Vec2::ZERO);
        state.open = true;
        state.screen_pos = egui::pos2(cursor_pos.x, cursor_pos.y);
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
    if !state.open { return Ok(()); }

    let egui_ctx = ctx.ctx_mut()?;
    consumed.0 = egui_ctx.is_pointer_over_area();

    egui::Area::new("action_menu".into())
        .fixed_pos(state.screen_pos)
        .order(egui::Order::Foreground)
        .show(egui_ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(140.0);
                ui.label(egui::RichText::new("Actions").strong());
                ui.separator();
                if ui.button("⚔ Attack").clicked() {
                    local_input.actions.push((Some(ActionIntent::BasicAttack), None));
                    state.open = false;
                }
                if ui.button("✦ Ability").clicked() {
                    local_input.actions.push((Some(ActionIntent::UseAbility(1)), None));
                    state.open = false;
                }
                if ui.button("🛡 Dodge").clicked() {
                    local_input.actions.push((Some(ActionIntent::Dodge), None));
                    state.open = false;
                }
                ui.separator();
                if ui.button("✕ Cancel").clicked() {
                    state.open = false;
                }
            });
        });

    Ok(())
}
