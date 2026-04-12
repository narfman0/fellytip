//! Dungeon area: spawns a boss NPC with a combat participant component.
//! Milestone 4 scaffold — boss abilities and room transitions follow later.

use bevy::prelude::*;
use fellytip_shared::{
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}},
    components::{Health, WorldPosition},
    world::ecology::RegionId,
};
use smol_str::SmolStr;
use uuid::Uuid;

use crate::plugins::combat::{CombatParticipant, ExperienceReward};

/// Marker: this entity is a dungeon boss.
#[derive(Component)]
pub struct BossNpc {
    #[allow(dead_code)]
    pub name: SmolStr,
    #[allow(dead_code)]
    pub region: RegionId,
}

/// Marker: this entity is inside the dungeon area.
#[derive(Component)]
pub struct InDungeon;

pub struct DungeonPlugin;

impl Plugin for DungeonPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_dungeon_boss);
    }
}

fn spawn_dungeon_boss(mut commands: Commands) {
    let id = CombatantId(Uuid::new_v4());
    commands.spawn((
        BossNpc {
            name: SmolStr::new("The Hollow King"),
            region: RegionId("dungeon_01".into()),
        },
        InDungeon,
        WorldPosition { x: 50.0, y: 50.0, z: 0.0 },
        Health { current: 500, max: 500 },
        CombatParticipant {
            id,
            interrupt_stack: InterruptStack::default(),
            class: CharacterClass::Warrior,
            level: 5,
            armor_class: 16, // chain mail (SRD: AC 16, no DEX)
            strength: 18,
            dexterity: 10,
            constitution: 18,
        },
        // CR 3 = 700 XP (SRD docs/dnd5e-srd-reference.md)
        ExperienceReward(700),
    ));
    tracing::info!("Dungeon boss 'The Hollow King' spawned");
}
