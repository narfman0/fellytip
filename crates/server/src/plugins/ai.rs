//! NPC AI plugin — re-evaluates faction goals and nudges NPC positions
//! each WorldSimSchedule tick (1 Hz).

use bevy::prelude::*;
use crate::plugins::persistence::Db;
use fellytip_shared::{
    combat::{interrupt::InterruptStack, types::{CharacterClass, CombatantId}},
    components::{Health, WorldPosition},
    world::{
        civilization::Settlements,
        faction::{Faction, FactionId, FactionResources, FactionGoal, pick_goal},
        ecology::RegionId,
        map::{MAP_HALF_WIDTH, MAP_HALF_HEIGHT},
    },
};
use smol_str::SmolStr;
use uuid::Uuid;

use crate::plugins::combat::{CombatParticipant, ExperienceReward};
use crate::plugins::world_sim::WorldSimSchedule;

/// Server-only component: which faction this NPC belongs to.
#[derive(Component)]
pub struct FactionMember(#[allow(dead_code)] pub FactionId);

/// Server-only component: current AI goal being pursued.
#[derive(Component)]
pub struct CurrentGoal(#[allow(dead_code)] pub Option<FactionGoal>);

/// Server-only component: home position used for bounded wander / future pathfinding.
#[derive(Component)]
pub struct HomePosition(#[allow(dead_code)] pub WorldPosition);

/// Server-only resource: all live factions.
#[derive(Resource, Default)]
pub struct FactionRegistry {
    pub factions: Vec<Faction>,
}

/// Number of NPC soldiers spawned per faction at startup.
const NPCS_PER_FACTION: usize = 3;

/// Fixed offsets (tile units) applied to each NPC spawn relative to the
/// faction's home settlement, so NPCs aren't stacked on top of each other.
const NPC_OFFSETS: [(f32, f32); NPCS_PER_FACTION] = [(0.0, 0.0), (2.0, 0.0), (0.0, 2.0)];

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FactionRegistry>();
        app.add_systems(WorldSimSchedule, (update_faction_goals, wander_npcs).chain());
        // spawn_faction_npcs and flush_factions_to_db are registered in MapGenPlugin's
        // Startup chain so they run after generate_world inserts the Settlements resource.
    }
}

/// Re-score and update the active goal for every faction.
fn update_faction_goals(mut registry: ResMut<FactionRegistry>) {
    for faction in &mut registry.factions {
        if let Some(top) = pick_goal(faction) {
            tracing::debug!(
                faction = %faction.name,
                goal = ?top,
                "Faction goal selected"
            );
        }
    }
}

/// Faction NPCs are stationary for now; real pathfinding comes later.
/// `HomePosition` is kept on each NPC for future bounded-wander use.
fn wander_npcs(query: Query<(&FactionMember, &HomePosition)>) {
    // Intentionally idle — removing the old off-map drift until pathfinding arrives.
    let _ = query;
}

/// Spawn `NPCS_PER_FACTION` guard NPCs for each faction at their nearest settlement.
/// Runs at Startup after `seed_factions` and after `MapGenPlugin` inserts `Settlements`.
pub fn spawn_faction_npcs(
    registry: Res<FactionRegistry>,
    settlements: Res<Settlements>,
    mut commands: Commands,
) {
    if settlements.0.is_empty() {
        tracing::warn!("No settlements available; skipping faction NPC spawn");
        return;
    }

    for (faction_idx, faction) in registry.factions.iter().enumerate() {
        // Assign each faction a home settlement by cycling through the list.
        let settlement = &settlements.0[faction_idx % settlements.0.len()];

        for (npc_idx, (ox, oy)) in NPC_OFFSETS.iter().enumerate() {
            // settlement.x/y are tile-space (0..MAP_WIDTH); convert to world-space.
            let pos = WorldPosition {
                x: settlement.x - MAP_HALF_WIDTH as f32 + ox,
                y: settlement.y - MAP_HALF_HEIGHT as f32 + oy,
                z: settlement.z,
            };
            commands.spawn((
                pos.clone(),
                Health { current: 20, max: 20 },
                CombatParticipant {
                    id: CombatantId(Uuid::new_v4()),
                    interrupt_stack: InterruptStack::default(),
                    class: CharacterClass::Warrior,
                    level: 1,
                    // Leather armour, DEX 10 → AC 11 (SRD: 11 + DEX mod)
                    armor_class: 11,
                    strength: 10,
                    dexterity: 10,
                    constitution: 10,
                },
                // CR 1/4 = 50 XP (docs/dnd5e-srd-reference.md)
                ExperienceReward(50),
                FactionMember(faction.id.clone()),
                CurrentGoal(None),
                HomePosition(pos),
            ));
            tracing::debug!(
                faction = %faction.name,
                settlement = %settlement.name,
                npc = npc_idx,
                "Faction NPC spawned"
            );
        }

        tracing::info!(
            faction = %faction.name,
            settlement = %settlement.name,
            count = NPCS_PER_FACTION,
            "Faction NPCs spawned"
        );
    }
}

/// Persist the current faction registry to SQLite so worldwatch can read it.
///
/// Runs at Startup after `seed_factions`. Uses the same block_on pattern as
/// `on_client_disconnected` in server/main.rs.
pub fn flush_factions_to_db(registry: Res<FactionRegistry>, db: Res<Db>) {
    let pool = db.pool().clone();
    let factions = registry.factions.clone();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime for faction flush");

    rt.block_on(async move {
        for faction in &factions {
            let id = faction.id.0.as_str().to_owned();
            let name = faction.name.as_str().to_owned();
            let resources = serde_json::to_string(&faction.resources)
                .unwrap_or_else(|_| "{}".to_owned());
            let territory = serde_json::to_string(&faction.territory)
                .unwrap_or_else(|_| "[]".to_owned());
            let goals = serde_json::to_string(&faction.goals)
                .unwrap_or_else(|_| "[]".to_owned());

            let res = sqlx::query(
                "INSERT OR REPLACE INTO factions (id, name, resources, territory, goals) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&id)
            .bind(&name)
            .bind(&resources)
            .bind(&territory)
            .bind(&goals)
            .execute(&pool)
            .await;

            match res {
                Ok(_) => tracing::debug!(faction = %name, "Faction flushed to DB"),
                Err(e) => tracing::warn!(faction = %name, "Faction flush failed: {e}"),
            }
        }
        tracing::info!(count = factions.len(), "Factions persisted to SQLite");
    });
}

/// Seed the faction registry with two starter factions for testing.
pub fn seed_factions(mut registry: ResMut<FactionRegistry>) {
    use std::collections::HashMap;
    registry.factions = vec![
        Faction {
            id: FactionId("wolves".into()),
            name: SmolStr::new("Iron Wolves"),
            disposition: HashMap::new(),
            goals: vec![FactionGoal::Survive, FactionGoal::RaidResource { resource_node_id: "mine_01".into() }],
            resources: FactionResources { food: 20.0, gold: 5.0, military_strength: 30.0 },
            territory: vec![RegionId("north".into())],
        },
        Faction {
            id: FactionId("guild".into()),
            name: SmolStr::new("Merchant Guild"),
            disposition: HashMap::new(),
            goals: vec![FactionGoal::FormAlliance { with: FactionId("wolves".into()), min_trust: 0.5 }, FactionGoal::Survive],
            resources: FactionResources { food: 80.0, gold: 200.0, military_strength: 10.0 },
            territory: vec![RegionId("south".into())],
        },
    ];
}
