//! NPC AI plugin — re-evaluates faction goals and nudges NPC positions
//! each WorldSimSchedule tick (1 Hz).

use bevy::prelude::*;
use fellytip_shared::{
    components::WorldPosition,
    world::faction::{Faction, FactionId, FactionResources, FactionGoal, pick_goal},
    world::ecology::RegionId,
};
use smol_str::SmolStr;

use crate::plugins::world_sim::WorldSimSchedule;

/// Server-only component: which faction this NPC belongs to.
#[derive(Component)]
pub struct FactionMember(#[allow(dead_code)] pub FactionId);

/// Server-only component: current AI goal being pursued.
#[derive(Component)]
pub struct CurrentGoal(#[allow(dead_code)] pub Option<FactionGoal>);

/// Server-only resource: all live factions.
#[derive(Resource, Default)]
pub struct FactionRegistry {
    pub factions: Vec<Faction>,
}

pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FactionRegistry>();
        app.add_systems(WorldSimSchedule, (update_faction_goals, wander_npcs).chain());
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

/// Simple wander: nudge each NPC's WorldPosition by ±1 in a fixed pattern.
/// Real pathfinding will replace this in milestone 1.
fn wander_npcs(mut query: Query<(&FactionMember, &mut WorldPosition, &mut CurrentGoal)>) {
    for (_member, mut pos, _goal) in query.iter_mut() {
        // Deterministic nudge — no RNG here; proper pathfinding comes later.
        pos.x = (pos.x + 1.0).rem_euclid(100.0);
    }
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
