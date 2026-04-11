//! World generation and history pre-simulation.
//!
//! On startup:
//! 1. Generate the tile map from a fixed seed using fBm + biome + river passes.
//! 2. Place surface and underground settlements.
//! 3. Assign territories and stamp the road network onto the map.
//! 4. Run `WorldSimSchedule` for [`HISTORY_WARP_TICKS`] ticks at warp speed so
//!    factions and ecology have meaningful state before the first player connects.

use bevy::prelude::*;
use fellytip_shared::{
    WORLD_SEED,
    world::{
        civilization::{assign_territories, generate_roads, generate_settlements, Settlements},
        map::generate_map,
    },
};

use crate::plugins::{ai::seed_factions, world_sim::WorldSimTick};

/// WorldSim ticks to run before the server accepts connections.
///
/// 200 ticks = 200 simulated seconds of world history (factions expand,
/// ecology reaches equilibrium, story events accumulate).
const HISTORY_WARP_TICKS: u64 = 200;

pub struct MapGenPlugin;

impl Plugin for MapGenPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            (generate_world, history_warp)
                .chain()
                .after(seed_factions),
        );
    }
}

/// Generate the world map + settlements and insert them as resources.
fn generate_world(mut commands: Commands) {
    tracing::info!(seed = WORLD_SEED, "Generating world map…");
    let mut map = generate_map(WORLD_SEED);

    let settlements = generate_settlements(&map, WORLD_SEED);
    tracing::info!(
        surface = settlements.iter().filter(|s| !matches!(s.kind, fellytip_shared::world::civilization::SettlementKind::UndergroundCity)).count(),
        underground = settlements.iter().filter(|s| matches!(s.kind, fellytip_shared::world::civilization::SettlementKind::UndergroundCity)).count(),
        "Settlements placed"
    );

    generate_roads(&mut map, &settlements);
    let road_count = map.road_tiles.iter().filter(|&&r| r).count();
    tracing::info!(road_count, "Road network stamped");

    let territory = assign_territories(&map, &settlements);
    let assigned = territory.iter().filter(|t| t.is_some()).count();
    tracing::info!(assigned, "Territory tiles assigned");

    commands.insert_resource(map);
    commands.insert_resource(Settlements(settlements));
    tracing::info!("World generation complete");
}

/// Run WorldSimSchedule [`HISTORY_WARP_TICKS`] times synchronously before
/// players can connect.  This "ages" the world: factions expand, ecology
/// reaches equilibrium, and story events accumulate.
fn history_warp(world: &mut World) {
    tracing::info!(ticks = HISTORY_WARP_TICKS, "Starting history warp…");
    for _ in 0..HISTORY_WARP_TICKS {
        world.resource_mut::<WorldSimTick>().0 += 1;
        world.run_schedule(crate::plugins::world_sim::WorldSimSchedule);
    }
    let tick = world.resource::<WorldSimTick>().0;
    tracing::info!(tick, "History warp complete — world is live");
}
