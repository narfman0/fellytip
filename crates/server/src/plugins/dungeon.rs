//! Dungeon area: spawns a boss NPC with a combat participant component.
//! Milestone 4 scaffold — boss abilities and room transitions follow later.

use bevy::prelude::*;
use fellytip_shared::{
    combat::types::CombatantId,
    components::WorldPosition,
    world::ecology::RegionId,
};
use smol_str::SmolStr;
use uuid::Uuid;

use crate::plugins::combat::{CombatParticipant, Health};
use fellytip_shared::combat::interrupt::InterruptStack;

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
        WorldPosition { x: 50.0, y: 50.0 },
        Health { current: 500, max: 500 },
        CombatParticipant {
            id,
            interrupt_stack: InterruptStack::default(),
            armor: 5,
            strength: 18,
        },
    ));
    tracing::info!("Dungeon boss 'The Hollow King' spawned");
}
