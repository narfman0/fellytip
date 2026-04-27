//! Class selection screen shown on first join (before the player spawns).
//!
//! Displays all 14 D&D 5e SRD classes in a scrollable egui window.
//! When the player clicks a class the plugin sends a `ChooseClassMessage` and
//! hides itself.  The server plugin listens for that message and spawns the
//! player entity with the appropriate stats.
//!
//! The screen is only shown while `ClassSelectionState::open == true`.  It is
//! closed automatically once the local player entity appears in the world.

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

fn auto_close_on_spawn(
    local_player: Query<Entity, With<fellytip_shared::components::Experience>>,
    mut state: ResMut<ClassSelectionState>,
) {
    if state.open && !local_player.is_empty() {
        state.open = false;
    }
}

struct ClassInfo {
    class: CharacterClass,
    label: &'static str,
    icon: &'static str,
    hit_die: &'static str,
    primary: &'static str,
    signature: &'static str,
    color: egui::Color32,
}

fn all_classes() -> Vec<ClassInfo> {
    vec![
        ClassInfo { class: CharacterClass::Warrior,   icon: "⚔",  label: "Warrior",   hit_die: "d10", primary: "Strength",     signature: "Extra Attack — strike twice per action.",           color: egui::Color32::from_rgb(200, 100,  50) },
        ClassInfo { class: CharacterClass::Barbarian,  icon: "🪓", label: "Barbarian",  hit_die: "d12", primary: "Strength",     signature: "Rage — advantage on STR checks, resist damage.",    color: egui::Color32::from_rgb(180,  60,  40) },
        ClassInfo { class: CharacterClass::Fighter,    icon: "🛡",  label: "Fighter",    hit_die: "d10", primary: "Strength/DEX", signature: "Action Surge — take a second action per rest.",     color: egui::Color32::from_rgb(140, 120,  80) },
        ClassInfo { class: CharacterClass::Paladin,    icon: "✝",  label: "Paladin",    hit_die: "d10", primary: "Strength/CHA", signature: "Divine Smite — spend spell slots for burst damage.", color: egui::Color32::from_rgb(220, 200, 100) },
        ClassInfo { class: CharacterClass::Rogue,      icon: "🗡",  label: "Rogue",      hit_die: "d8",  primary: "Dexterity",   signature: "Sneak Attack — bonus damage on flanked hits.",       color: egui::Color32::from_rgb( 80, 180,  80) },
        ClassInfo { class: CharacterClass::Ranger,     icon: "🏹",  label: "Ranger",     hit_die: "d10", primary: "Dexterity",   signature: "Hunter's Mark — track and deal extra damage.",       color: egui::Color32::from_rgb( 60, 160,  60) },
        ClassInfo { class: CharacterClass::Monk,       icon: "👊",  label: "Monk",       hit_die: "d8",  primary: "Dex / WIS",   signature: "Flurry of Blows — spend ki for rapid unarmed hits.", color: egui::Color32::from_rgb(200, 160,  40) },
        ClassInfo { class: CharacterClass::Cleric,     icon: "☩",  label: "Cleric",     hit_die: "d8",  primary: "Wisdom",      signature: "Channel Divinity — turn undead or heal allies.",     color: egui::Color32::from_rgb(240, 240, 180) },
        ClassInfo { class: CharacterClass::Druid,      icon: "🌿",  label: "Druid",      hit_die: "d8",  primary: "Wisdom",      signature: "Wild Shape — transform into beasts.",                color: egui::Color32::from_rgb( 80, 160,  60) },
        ClassInfo { class: CharacterClass::Bard,       icon: "🎵",  label: "Bard",       hit_die: "d8",  primary: "Charisma",    signature: "Bardic Inspiration — boost ally rolls.",             color: egui::Color32::from_rgb(180,  80, 200) },
        ClassInfo { class: CharacterClass::Warlock,    icon: "👁",  label: "Warlock",    hit_die: "d8",  primary: "Charisma",    signature: "Eldritch Blast — reliable ranged force damage.",     color: egui::Color32::from_rgb(100,  40, 160) },
        ClassInfo { class: CharacterClass::Sorcerer,   icon: "✨",  label: "Sorcerer",   hit_die: "d6",  primary: "Charisma",    signature: "Metamagic — shape spells for maximum effect.",       color: egui::Color32::from_rgb(220,  80, 120) },
        ClassInfo { class: CharacterClass::Mage,       icon: "✦",  label: "Mage",       hit_die: "d6",  primary: "Intelligence", signature: "Arcane Surge — burst spell damage.",                color: egui::Color32::from_rgb(100, 140, 240) },
        ClassInfo { class: CharacterClass::Wizard,     icon: "📚",  label: "Wizard",     hit_die: "d6",  primary: "Intelligence", signature: "Arcane Recovery — regain spell slots on rest.",     color: egui::Color32::from_rgb( 80, 120, 220) },
    ]
}

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
        .fixed_size([480.0, 520.0])
        .show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Welcome to Fellytip");
                ui.label("Select a class to begin your adventure.");
            });
            ui.separator();
            ui.add_space(4.0);

            egui::ScrollArea::vertical()
                .max_height(440.0)
                .show(ui, |ui| {
                    for info in all_classes() {
                        egui::Frame::new()
                            .fill(egui::Color32::from_rgba_unmultiplied(
                                info.color.r() / 4,
                                info.color.g() / 4,
                                info.color.b() / 4,
                                200,
                            ))
                            .inner_margin(egui::Margin::same(8))
                            .corner_radius(egui::CornerRadius::same(4))
                            .show(ui, |ui| {
                                ui.set_min_width(450.0);
                                ui.horizontal(|ui| {
                                    ui.colored_label(
                                        info.color,
                                        format!("{}  {}", info.icon, info.label),
                                    );
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if ui.button("▶ Play").clicked() {
                                            writer.write(ChooseClassMessage { class: info.class });
                                            state.open = false;
                                        }
                                    });
                                });
                                ui.label(format!(
                                    "Hit die: {}  |  Primary: {}",
                                    info.hit_die, info.primary
                                ));
                                ui.label(egui::RichText::new(info.signature).italics().weak());
                            });
                        ui.add_space(4.0);
                    }
                });
        });

    Ok(())
}
