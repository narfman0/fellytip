pub mod basic_movement;
pub mod combat_resolves;
pub mod movement_e2e;
pub mod npc_spawn_with_dm;
pub mod player_moves;
pub mod underground_e2e;
pub mod war_party_e2e;

use anyhow::Result;

pub trait Scenario {
    fn name(&self) -> &str;
    fn run(&self) -> Result<()>;
}

pub fn all_scenarios() -> Vec<Box<dyn Scenario>> {
    vec![
        Box::new(basic_movement::BasicMovement),
        Box::new(combat_resolves::CombatResolves),
        Box::new(player_moves::PlayerMoves),
        Box::new(npc_spawn_with_dm::NpcSpawnWithDm),
        Box::new(war_party_e2e::WarPartyE2e),
        Box::new(underground_e2e::UndergroundE2e),
        Box::new(movement_e2e::MovementE2e),
    ]
}

pub fn find_scenario(name: &str) -> Option<Box<dyn Scenario>> {
    all_scenarios().into_iter().find(|s| s.name() == name)
}
