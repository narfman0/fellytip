//! NPC AI plugin — re-evaluates faction goals and nudges NPC positions
//! each WorldSimSchedule tick (1 Hz).
//!
//! # Three-tier pathfinding LOD
//!
//! All pathfinding is gated on the entity's `SimTier` (see `interest::effective_zone`):
//!
//! | Tier   | Individual NPCs (wander_npcs)         | War parties (march_war_parties)        | Separation (war_party_separation) |
//! |--------|---------------------------------------|----------------------------------------|-----------------------------------|
//! | Hot    | A* replan every 2 ticks, full speed   | Flow-field sampling, full MARCH_SPEED  | Pairwise repulsion                |
//! | Warm   | A* replan every 8 ticks, 0.25× speed  | Flow-field sampling, 0.25× speed       | Pairwise repulsion                |
//! | Frozen | Linear march toward home, 0.05× speed | Linear march to target, 0.05× speed   | Fixed centroid-offset formation   |
//!
//! Frozen entities always reach their goal (macro-correct behavior) via linear march;
//! expensive A* and flow-field sampling are skipped entirely for out-of-range chunks.

pub mod battle;
pub mod goal;
pub mod pathfinding;
pub mod population;
pub mod surface_danger;

use bevy::prelude::*;
use fellytip_shared::protocol::{BattleAttackMsg, BattleEndMsg, BattleStartMsg};
use fellytip_shared::world::faction::{FactionId, FactionGoal, FactionRelations, NpcRank, PlayerReputationMap, Faction};
use fellytip_shared::world::population::SettlementPopulation;
use fellytip_shared::components::WorldPosition;
use std::collections::HashMap;
use uuid::Uuid;

use crate::plugins::nav::FlowField;
use crate::plugins::world_sim::{UndergroundSimSchedule, WorldSimSchedule};

// Re-export systems and types from sub-modules so existing callers don't break.
pub use battle::{ActiveBattle, BattleHistory, BattleRecord, run_battle_rounds};
pub use goal::{update_faction_alerts, update_faction_goals};
pub use pathfinding::{
    advance_zone_parties, march_war_parties, sync_player_standings,
    update_war_party_player_targets, war_party_separation, wander_npcs,
};
pub use population::{
    accumulate_underground_pressure, age_npcs_system,
    check_war_party_formation, deliver_underground_signals, flush_factions_to_db,
    init_population_state, seed_factions, spawn_faction_npcs,
    spawn_underground_raid, tick_population_system, FormWarPartyEvent,
};
pub use surface_danger::{
    update_danger_levels, spawn_bandit_groups, spawn_portal_horrors,
    resolve_warfront_events, update_threat_registry, auto_generate_raids,
    BanditCamp, DangerLevel, DangerTier, PortalHorror,
    ThreatRegistry, WarfrontRegistry,
};

// ── Shared types (referenced by multiple sub-modules) ──────────────────────────

/// Alert level for a faction after a war event.
///
/// Raised when the faction wins or loses a battle (`BattleEndMsg`).
/// Decays back to `Calm` after `ALERT_DECAY_TICKS` world-sim ticks.
/// NPCs check this via `FactionAlertState` during the wander system —
/// when their faction is `Alerted` they patrol a larger radius and at
/// higher speed, making the faction visibly more dangerous.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FactionAlertLevel {
    /// Normal wandering behaviour.
    Calm,
    /// Post-battle state: expanded patrol radius, increased speed.
    Alerted,
}

/// Per-faction alert state with a decay counter.
#[derive(Clone, Debug)]
pub struct FactionAlert {
    pub level: FactionAlertLevel,
    /// World-sim ticks remaining before the alert decays back to Calm.
    pub ticks_remaining: u32,
}

/// Resource: alert levels for all factions, keyed by `FactionId`.
#[derive(Resource, Default)]
pub struct FactionAlertState {
    pub alerts: HashMap<FactionId, FactionAlert>,
}

impl FactionAlertState {
    /// Number of world-sim ticks an alert persists before decaying (5 minutes at 1 Hz).
    pub const ALERT_DECAY_TICKS: u32 = 300;

    /// Raise a faction to `Alerted`, resetting the decay counter.
    pub fn raise(&mut self, faction_id: &FactionId) {
        self.alerts.insert(
            faction_id.clone(),
            FactionAlert {
                level: FactionAlertLevel::Alerted,
                ticks_remaining: Self::ALERT_DECAY_TICKS,
            },
        );
    }

    /// Returns true when the faction is currently alerted.
    pub fn is_alerted(&self, faction_id: &FactionId) -> bool {
        self.alerts
            .get(faction_id)
            .is_some_and(|a| a.level == FactionAlertLevel::Alerted)
    }

    /// Decay all alert counters by one tick; remove expired entries.
    pub fn tick_decay(&mut self) {
        self.alerts.retain(|_, alert| {
            if alert.ticks_remaining == 0 {
                return false;
            }
            alert.ticks_remaining -= 1;
            if alert.ticks_remaining == 0 {
                alert.level = FactionAlertLevel::Calm;
            }
            alert.ticks_remaining > 0
        });
    }
}

/// Server-only component: which faction this NPC belongs to.
#[derive(Component)]
pub struct FactionMember(#[allow(dead_code)] pub FactionId);

/// Server-only component: current AI goal being pursued.
#[derive(Component)]
pub struct CurrentGoal(#[allow(dead_code)] pub Option<FactionGoal>);

/// Server-only component: home position used for bounded wander / future pathfinding.
#[derive(Component)]
pub struct HomePosition(pub WorldPosition);

/// Server-only component: NPC rank for kill-penalty calculation.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct FactionNpcRank(pub NpcRank);

/// Server-only resource: all live factions.
#[derive(Resource, Default)]
pub struct FactionRegistry {
    pub factions: Vec<Faction>,
}

/// Tags an NPC as part of an active war party marching toward a target.
/// Server-only — never replicated, but registered for reflection so ralph
/// scenarios can observe war-party membership via `bevy/query`.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct WarPartyMember {
    pub target_settlement_id: Uuid,
    pub target_x: f32,
    pub target_y: f32,
    /// Faction that dispatched this war party. Stored so the spawned
    /// `ActiveBattle` (and downstream `BattleRecord`) has the correct
    /// attacker — we cannot reliably recover it from `FactionMember` alone
    /// because `march_war_parties` doesn't query that component.
    ///
    /// `FactionId` doesn't derive `Reflect`, so it's skipped during reflection.
    #[reflect(ignore)]
    pub attacker_faction: FactionId,
    /// When set, this war party is hunting the given player entity rather than
    /// marching to the settlement.  `target_x`/`target_y` are updated each tick
    /// by `update_war_party_player_targets` to follow the player's position.
    pub player_target: Option<Entity>,
    /// Zone the war party member currently occupies. Defaults to the overworld.
    /// Reflect-skipped because ZoneId is in shared::world::zone and doesn't
    /// satisfy the reflection derive requirements downstream.
    #[reflect(ignore)]
    pub current_zone: fellytip_shared::world::zone::ZoneId,
    /// Pre-computed zone hop path this party is traversing. Empty = overworld-only.
    #[reflect(ignore)]
    pub zone_route: Vec<fellytip_shared::world::zone::ZoneId>,
}

/// Per-settlement mutable population state — one entry per settlement.
#[derive(Resource, Default)]
pub struct FactionPopulationState {
    pub settlements: HashMap<Uuid, SettlementPopulation>,
}

/// Background pressure score for the underground faction's surface raids.
///
/// Accumulates slowly on `UndergroundSimSchedule` (0.1 Hz). When it crosses
/// configured thresholds it emits environmental signals (`StoryEvent`s); when
/// it peaks the raid spawn system converts it into a concrete war party.
///
/// * `score`: 0.0 = calm, 1.0 = imminent raid
/// * `last_raid_tick`: `WorldSimTick` when the last raid party spawned
/// * `thresholds_crossed`: bitmask of thresholds currently latched (bit 0 = 0.4
///   distant signal, bit 1 = 0.7 imminent signal). Uses hysteresis: bits are
///   set when crossed upward and cleared when the score drops back below 0.4.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct UndergroundPressure {
    pub score: f32,
    pub last_raid_tick: u64,
    pub thresholds_crossed: u8,
}

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FactionRegistry>()
            .init_resource::<PlayerReputationMap>()
            .init_resource::<FactionRelations>()
            .init_resource::<FactionPopulationState>()
            .init_resource::<FlowField>()
            .init_resource::<BattleHistory>()
            .init_resource::<UndergroundPressure>()
            .init_resource::<FactionAlertState>()
            .init_resource::<DangerLevel>()
            .init_resource::<ThreatRegistry>()
            .init_resource::<WarfrontRegistry>()
            .add_message::<FormWarPartyEvent>()
            .register_type::<FactionNpcRank>()
            .register_type::<WarPartyMember>()
            .register_type::<fellytip_shared::world::civilization::AbandonedSettlement>()
            .register_type::<fellytip_shared::world::civilization::RuinsTile>()
            .register_type::<fellytip_shared::components::EconomicRole>()
            .add_message::<BattleStartMsg>()
            .add_message::<BattleEndMsg>()
            .add_message::<BattleAttackMsg>();
        app.add_systems(
            WorldSimSchedule,
            (
                update_faction_alerts,
                update_faction_goals,
                tick_population_system,
                age_npcs_system,
                check_war_party_formation,
                update_war_party_player_targets,
                advance_zone_parties,
                spawn_underground_raid,
                march_war_parties,
                war_party_separation,
                run_battle_rounds,
                wander_npcs,
                sync_player_standings,
                // Surface danger systems (issues #117-#121, #97)
                update_danger_levels,
                spawn_bandit_groups,
                spawn_portal_horrors,
                resolve_warfront_events,
                update_threat_registry,
                auto_generate_raids,
            ).chain(),
        );
        app.add_systems(
            UndergroundSimSchedule,
            (
                accumulate_underground_pressure,
                deliver_underground_signals,
            ).chain(),
        );
        // spawn_faction_npcs, init_population_state, and flush_factions_to_db are
        // registered in MapGenPlugin's Startup chain so they run after
        // generate_world inserts the Settlements resource.
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use smol_str::SmolStr;

    fn faction_id(s: &str) -> FactionId {
        FactionId(SmolStr::new(s))
    }

    #[test]
    fn faction_alert_raise_marks_alerted() {
        let mut state = FactionAlertState::default();
        let fid = faction_id("iron_wolves");
        assert!(!state.is_alerted(&fid));
        state.raise(&fid);
        assert!(state.is_alerted(&fid));
    }

    #[test]
    fn faction_alert_decays_after_ticks() {
        let mut state = FactionAlertState::default();
        let fid = faction_id("ash_covenant");
        state.raise(&fid);
        // Tick ALERT_DECAY_TICKS times — should be calm afterward.
        for _ in 0..FactionAlertState::ALERT_DECAY_TICKS {
            state.tick_decay();
        }
        assert!(!state.is_alerted(&fid));
    }

    #[test]
    fn faction_alert_raise_resets_decay_counter() {
        let mut state = FactionAlertState::default();
        let fid = faction_id("merchant_guild");
        state.raise(&fid);
        // Partial decay
        for _ in 0..50 {
            state.tick_decay();
        }
        // Re-raise resets counter
        state.raise(&fid);
        assert!(state.is_alerted(&fid));
        // Should still be alerted after ALERT_DECAY_TICKS - 50 more ticks
        for _ in 0..(FactionAlertState::ALERT_DECAY_TICKS - 50) {
            state.tick_decay();
        }
        // Was just raised, so we still have ALERT_DECAY_TICKS remaining before expiry
        // (50 ticks elapsed after the raise), so still alerted.
        assert!(state.is_alerted(&fid));
    }

    #[test]
    fn unknown_faction_is_not_alerted() {
        let state = FactionAlertState::default();
        assert!(!state.is_alerted(&faction_id("nonexistent")));
    }
}
