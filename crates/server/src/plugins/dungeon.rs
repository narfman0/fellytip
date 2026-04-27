//! Dungeon area: spawns a boss NPC with phased combat abilities.
//!
//! # Boss phase transitions
//!
//! The Hollow King has three phases gated by HP percentage:
//!
//! | Phase | HP threshold | Behaviour change |
//! |-------|-------------|-----------------|
//! | 1     | > 50 %      | Normal attacks (ability 1: StrongAttack) |
//! | 2     | 25 – 50 %   | Rage: ability roll bonus +2, applies "enraged" status to self |
//! | 3     | < 25 %      | Frenzy: attacks twice per trigger, applies "weakened" to targets |
//!
//! Phase transitions are one-way and recorded on the `BossPhase` component so
//! the ECS bridge can route the boss to the correct ability in `initiate_attacks`.

use bevy::prelude::*;
use fellytip_shared::{
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}},
    components::{Health, WorldPosition},
    world::ecology::RegionId,
};
use smol_str::SmolStr;
use uuid::Uuid;

use crate::plugins::combat::{CombatParticipant, ExperienceReward};

// ── Phase marker ─────────────────────────────────────────────────────────────

/// Current combat phase of the dungeon boss.
///
/// Transitions are strictly one-way: Phase1 → Phase2 → Phase3.
/// The ECS bridge reads this to select the boss's ability_id for each attack.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub enum BossPhase {
    /// > 50 % HP — normal attacks (ability 1: StrongAttack, 2×d8).
    Phase1,
    /// 25–50 % HP — rage: damage bonus, applies "enraged" self-buff.
    Phase2,
    /// < 25 % HP — frenzy: uses ability 4 (DoubleFrenzy), hits twice.
    Phase3,
}

impl BossPhase {
    /// Map phase to the ability_id resolved by `resolve_ability`.
    pub fn ability_id(&self) -> u8 {
        match self {
            BossPhase::Phase1 => 1, // StrongAttack
            BossPhase::Phase2 => 5, // BossRage
            BossPhase::Phase3 => 6, // BossFrenzy
        }
    }
}

// ── Other markers ─────────────────────────────────────────────────────────────

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

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct DungeonPlugin;

impl Plugin for DungeonPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_dungeon_boss)
           .add_systems(FixedUpdate, tick_boss_phase_transitions);
    }
}

fn spawn_dungeon_boss(mut commands: Commands) {
    use fellytip_shared::world::faction::NpcRank;
    let id = CombatantId(Uuid::new_v4());
    // The Hollow King is a Boss-rank Fighter — max STR primary, heavy melee spec.
    let boss_class = CharacterClass::Fighter;
    let boss_scores = fellytip_shared::components::AbilityScores::for_class(
        &boss_class,
        NpcRank::Boss,
    );
    commands.spawn((
        BossNpc {
            name: SmolStr::new("The Hollow King"),
            region: RegionId("dungeon_01".into()),
        },
        InDungeon,
        BossPhase::Phase1,
        WorldPosition { x: 50.0, y: 50.0, z: 0.0 },
        Health { current: 500, max: 500 },
        CombatParticipant {
            id,
            interrupt_stack: InterruptStack::default(),
            class: boss_class,
            level: 5,
            armor_class: 16, // chain mail (SRD: AC 16, no DEX)
            strength: boss_scores.strength as i32,
            dexterity: boss_scores.dexterity as i32,
            constitution: boss_scores.constitution as i32,
            intelligence: boss_scores.intelligence as i32,
            wisdom: boss_scores.wisdom as i32,
            charisma: boss_scores.charisma as i32,
        },
        // CR 3 = 700 XP (SRD docs/dnd5e-srd-reference.md)
        ExperienceReward(700),
    ));
    tracing::info!("Dungeon boss 'The Hollow King' spawned (Fighter, Boss rank)");
}

// ── Phase transition system ───────────────────────────────────────────────────

/// Advance boss phase based on current HP percentage.
///
/// Phase transitions are strictly one-way: 1 → 2 → 3.
/// Logs a warning when a threshold is crossed so it's visible in the server log.
fn tick_boss_phase_transitions(
    mut bosses: Query<(&Health, &mut BossPhase), With<BossNpc>>,
) {
    for (health, mut phase) in &mut bosses {
        let pct = if health.max > 0 {
            health.current as f32 / health.max as f32
        } else {
            0.0
        };

        let new_phase = if pct <= 1.0 / 3.0 {
            BossPhase::Phase3
        } else if pct <= 2.0 / 3.0 {
            BossPhase::Phase2
        } else {
            BossPhase::Phase1
        };

        if new_phase != *phase {
            tracing::warn!(
                old_phase = ?*phase,
                new_phase = ?new_phase,
                hp = health.current,
                max_hp = health.max,
                "Boss phase transition!"
            );
            *phase = new_phase;
        }
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn health(current: i32, max: i32) -> Health {
        Health { current, max }
    }

    #[test]
    fn phase1_above_two_thirds() {
        // 401/600 ≈ 66.8 % → Phase1 (less than 1/3 HP lost)
        assert_eq!(phase_for_hp(&health(401, 600)), BossPhase::Phase1);
        // 700/900 ≈ 77.8 % → Phase1
        assert_eq!(phase_for_hp(&health(700, 900)), BossPhase::Phase1);
    }

    #[test]
    fn phase2_between_one_third_and_two_thirds() {
        // 300/600 = 50 % → Phase2
        assert_eq!(phase_for_hp(&health(300, 600)), BossPhase::Phase2);
        // Exactly 2/3 boundary (400/600) is inclusive → Phase2
        assert_eq!(phase_for_hp(&health(400, 600)), BossPhase::Phase2);
        // Just above 2/3 → Phase1
        assert_eq!(phase_for_hp(&health(401, 600)), BossPhase::Phase1);
    }

    #[test]
    fn phase3_below_one_third() {
        // 150/600 = 25 % → Phase3 (more than 2/3 HP lost)
        assert_eq!(phase_for_hp(&health(150, 600)), BossPhase::Phase3);
        // Exactly 1/3 boundary (200/600) is inclusive → Phase3
        assert_eq!(phase_for_hp(&health(200, 600)), BossPhase::Phase3);
        // Just above 1/3 → Phase2
        assert_eq!(phase_for_hp(&health(201, 600)), BossPhase::Phase2);
    }

    #[test]
    fn ability_ids_are_distinct() {
        assert_ne!(BossPhase::Phase1.ability_id(), BossPhase::Phase2.ability_id());
        assert_ne!(BossPhase::Phase2.ability_id(), BossPhase::Phase3.ability_id());
        assert_ne!(BossPhase::Phase1.ability_id(), BossPhase::Phase3.ability_id());
    }

    /// Helper: compute the phase for a given health (same logic as the system).
    fn phase_for_hp(health: &Health) -> BossPhase {
        let pct = if health.max > 0 {
            health.current as f32 / health.max as f32
        } else {
            0.0
        };
        if pct <= 1.0 / 3.0 {
            BossPhase::Phase3
        } else if pct <= 2.0 / 3.0 {
            BossPhase::Phase2
        } else {
            BossPhase::Phase1
        }
    }
}
