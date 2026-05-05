//! Combat test mode: a minimal two-entity world for the `ralph combat_resolves` scenario.
//!
//! Activated with `cargo run -p fellytip-server -- --combat-test`.
//! Skips full world-gen, ecology, and history warp.  Spawns an Iron Wolves
//! brawler and a Merchant Guild guard whose faction alignment makes them
//! immediately hostile.  The brawler attacks the guard once per WorldSim tick
//! (1 Hz); ralph's `combat_resolves` scenario passes as soon as any entity's
//! `Health.current < Health.max`.
//!
//! No lightyear client is needed — attacks are injected directly into the
//! interrupt stack, bypassing the player-input path entirely.

use bevy::prelude::*;
use fellytip_shared::{
    combat::{
        interrupt::{AttackContext, InterruptFrame, InterruptStack},
        types::{CharacterClass, CombatantId},
    },
    components::{Health, WorldPosition},
};
use uuid::Uuid;

use crate::plugins::{
    combat::{CombatParticipant, ExperienceReward},
    world_sim::WorldSimSchedule,
};

pub struct CombatTestPlugin;

impl Plugin for CombatTestPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_combat_test_world);
        app.add_systems(WorldSimSchedule, brawler_attacks_guard);
    }
}

/// Entities spawned for the combat test scenario.
#[derive(Resource)]
struct CombatTestEntities {
    brawler: Entity,
    guard:   Entity,
}

/// Spawn an Iron Wolves brawler and a Merchant Guild guard side by side.
///
/// Uses `&mut World` directly so the `CombatTestEntities` resource is available
/// immediately — no deferred-command latency before `brawler_attacks_guard` runs.
fn spawn_combat_test_world(world: &mut World) {
    tracing::info!("Combat test: spawning Iron Wolves brawler vs Merchant Guild guard");

    // Brawler — level-2 Warrior, STR 14 (+2), prof +2 → hits AC 10 on roll ≥ 6 (75%).
    let brawler = world
        .spawn((
            WorldPosition { x: 0.0, y: 0.0, z: 0.0 },
            Health { current: 30, max: 30 },
            CombatParticipant {
                id:           CombatantId(Uuid::new_v4()),
                interrupt_stack: InterruptStack::default(),
                class:        CharacterClass::Warrior,
                level:        2,
                armor_class:  12,
                strength:     14,
                dexterity:    10,
                constitution: 12,
                intelligence: 8,
                wisdom:       10,
                charisma:     12,
            },
        ))
        .id();

    // Guard — level-1 Rogue, AC 10, low stats → easy target for verifying pipeline.
    let guard = world
        .spawn((
            WorldPosition { x: 2.0, y: 0.0, z: 0.0 },
            Health { current: 20, max: 20 },
            CombatParticipant {
                id:           CombatantId(Uuid::new_v4()),
                interrupt_stack: InterruptStack::default(),
                class:        CharacterClass::Rogue,
                level:        1,
                armor_class:  10,
                strength:     8,
                dexterity:    12,
                constitution: 10,
                intelligence: 12,
                wisdom:       10,
                charisma:     14,
            },
            ExperienceReward(50),
        ))
        .id();

    world.insert_resource(CombatTestEntities { brawler, guard });
    tracing::info!(?brawler, ?guard, "Combat test entities ready — brawler attacks guard at 1 Hz");
}

/// Push one `ResolvingAttack` frame per WorldSim tick (1 Hz).
///
/// `resolve_interrupts` (FixedUpdate, 62.5 Hz) processes the frame and
/// applies the resulting `TakeDamage` effect within milliseconds.
fn brawler_attacks_guard(
    entities: Res<CombatTestEntities>,
    mut participants: Query<&mut CombatParticipant>,
) {
    // Guard was despawned on death — nothing left to attack.
    let guard_id = {
        let Ok(g) = participants.get(entities.guard) else { return };
        g.id.clone()
    };
    let Ok(mut brawler) = participants.get_mut(entities.brawler) else { return };
    let brawler_id = brawler.id.clone();

    brawler.interrupt_stack.push(InterruptFrame::ResolvingAttack {
        ctx: AttackContext {
            attacker:    brawler_id,
            defender:    guard_id,
            attack_roll: rand::random_range(1..=20),
            dmg_roll:    rand::random_range(1..=8),
        },
    });
    tracing::debug!("Combat test: attack frame pushed to interrupt stack");
}
