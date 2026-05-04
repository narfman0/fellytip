//! World generation and history pre-simulation.
//!
//! On startup:
//! 1. Generate (or load from the unified binary cache) all static world data.
//! 2. Seed ecology from world map biomes.
//! 3. Spawn faction guard NPCs at their home settlements.
//! 4. Run `WorldSimSchedule` for [`HISTORY_WARP_TICKS`] ticks at warp speed so
//!    factions and ecology have meaningful state before the first player connects.
//!
//! ## Unified world gen cache
//!
//! All static derived data — tile map, settlements, buildings, zone graph, and
//! nav grids — is written to a single bincode file (`world_{seed}_{w}x{h}.bin`)
//! on first generation.  Subsequent startups load the whole bundle in one read,
//! skipping all generation work.
//!
//! The file is invalidated (and fully regenerated) when:
//! - `WORLD_CACHE_VERSION` is bumped (any algorithm change)
//! - The seed or map dimensions change
//!
//! **Bump `WORLD_CACHE_VERSION` whenever you change `generate_map`,
//! `generate_settlements_full`, `generate_buildings`, `generate_zones`, or any
//! nav grid builder.**

use std::path::PathBuf;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use fellytip_shared::{
    WORLD_SEED,
    components::{EntityKind, WorldPosition},
    world::{
        civilization::{
            apply_building_tiles, generate_buildings, generate_roads,
            generate_settlements_full, Buildings, Settlements,
        },
        map::{generate_map, generate_spawn_points, WorldMap, MAP_HALF_WIDTH, MAP_HALF_HEIGHT},
        zone::{generate_zones, Zone, ZoneKind, ZoneParent, ZoneRegistry, ZoneTopology, OVERWORLD_ZONE},
    },
};

use crate::plugins::{
    ai::{flush_factions_to_db, init_population_state, seed_factions, spawn_faction_npcs},
    ecology::seed_ecology,
    nav::{build_nav_grid, build_nav_grid_from_map, build_zone_nav_grids_from_registry, NavGrid, ZoneNavGrids},
    world_sim::WorldSimTick,
};

// ── Cache versioning ──────────────────────────────────────────────────────────

/// Bump this whenever any generation algorithm changes so stale cache files are
/// automatically discarded and regenerated.
const WORLD_CACHE_VERSION: u32 = 1;

/// Runtime map generation configuration — insert before [`MapGenPlugin`] is added.
#[derive(Resource, Reflect, Clone)]
#[reflect(Resource)]
pub struct MapGenConfig {
    pub seed:               u64,
    pub width:              usize,
    pub height:             usize,
    /// WorldSim ticks to run before the server accepts connections.
    /// Set to 0 for fastest startup (saves ~300–500 ms). Default: 10.
    pub history_warp_ticks: u64,
    /// NPC soldiers spawned per faction at startup. Default: 3.
    pub npcs_per_faction:   usize,
}

// ── Cache bundle ──────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct WorldGenCache {
    version:         u32,
    seed:            u64,
    width:           usize,
    height:          usize,
    map:             WorldMap,
    settlements:     Settlements,
    buildings:       Buildings,
    zone_registry:   ZoneRegistry,
    zone_topology:   ZoneTopology,
    nav_grid:        NavGrid,
    zone_nav_grids:  ZoneNavGrids,
}

// ── File helpers ──────────────────────────────────────────────────────────────

fn cache_path(seed: u64, width: usize, height: usize) -> PathBuf {
    PathBuf::from(format!("world_{seed}_{width}x{height}.bin"))
}

fn try_load_cache(path: &PathBuf) -> Option<WorldGenCache> {
    let bytes = std::fs::read(path).ok()?;
    match bincode::deserialize::<WorldGenCache>(&bytes) {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::warn!(path = %path.display(), "World cache corrupt or incompatible: {e}");
            None
        }
    }
}

fn save_cache(cache: &WorldGenCache) {
    let path = cache_path(cache.seed, cache.width, cache.height);
    match bincode::serialize(cache) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&path, &bytes) {
                tracing::warn!(path = %path.display(), "Failed to write world cache: {e}");
            } else {
                tracing::info!(path = %path.display(), bytes = bytes.len(), "World cache saved");
            }
        }
        Err(e) => tracing::warn!("Failed to serialise world cache: {e}"),
    }
}

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct MapGenPlugin;

impl Plugin for MapGenPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            (generate_world, ApplyDeferred, build_nav_grid, populate_zones, ApplyDeferred, seed_ecology, spawn_faction_npcs, init_population_state, spawn_settlement_markers, history_warp, flush_factions_to_db)
                .chain()
                .after(seed_factions),
        );
    }
}

// ── Full generation pipeline ──────────────────────────────────────────────────

fn run_generation(config: &MapGenConfig) -> WorldGenCache {
    tracing::info!(seed = config.seed, width = config.width, height = config.height, "Generating world…");

    let mut map = generate_map(config.seed, config.width, config.height);
    let settlements = generate_settlements_full(&mut map, config.seed);
    generate_roads(&mut map, &settlements);
    let buildings = generate_buildings(&settlements, &map, config.seed);
    apply_building_tiles(&buildings, &mut map);
    map.spawn_points = generate_spawn_points(&map);

    let road_count = map.road_tiles.iter().filter(|&&r| r).count();
    tracing::info!(road_count, count = buildings.len(), spawns = map.spawn_points.len(), "Map pipeline complete");

    let (zone_registry, zone_topology) = build_zone_graph(&buildings, config.seed);
    let nav_grid = build_nav_grid_from_map(&map);
    let zone_nav_grids = build_zone_nav_grids_from_registry(&zone_registry);

    WorldGenCache {
        version: WORLD_CACHE_VERSION,
        seed: config.seed,
        width: config.width,
        height: config.height,
        map,
        settlements: Settlements(settlements),
        buildings: Buildings(buildings),
        zone_registry,
        zone_topology,
        nav_grid,
        zone_nav_grids,
    }
}

fn insert_cache(commands: &mut Commands, cache: WorldGenCache) {
    let settlement_count = cache.settlements.0.len();
    let building_count   = cache.buildings.0.len();
    let zone_count       = cache.zone_registry.zones.len();
    let portal_count     = cache.zone_topology.portals.len();
    commands.insert_resource(cache.map);
    commands.insert_resource(cache.settlements);
    commands.insert_resource(cache.buildings);
    commands.insert_resource(cache.zone_registry);
    commands.insert_resource(cache.zone_topology);
    commands.insert_resource(cache.nav_grid);
    commands.insert_resource(cache.zone_nav_grids);
    tracing::info!(settlement_count, building_count, zone_count, portal_count, "World resources inserted");
}

// ── Bevy systems ──────────────────────────────────────────────────────────────

fn generate_world(mut commands: Commands, config: Res<MapGenConfig>) {
    let path = cache_path(config.seed, config.width, config.height);

    if let Some(cache) = try_load_cache(&path) {
        if cache.version == WORLD_CACHE_VERSION
            && cache.seed   == config.seed
            && cache.width  == config.width
            && cache.height == config.height
        {
            tracing::info!(path = %path.display(), "World loaded from cache");
            insert_cache(&mut commands, cache);
            return;
        }
        tracing::warn!(
            cached_version = cache.version,
            current_version = WORLD_CACHE_VERSION,
            "World cache invalid — regenerating"
        );
    } else {
        tracing::info!("No valid world cache — generating");
    }

    let cache = run_generation(&config);
    save_cache(&cache);
    insert_cache(&mut commands, cache);
}

/// Generate the zone graph from buildings and apply world-space anchor fixup.
///
/// Kept as a standalone function so `populate_zones` (Bevy system) and
/// `generate_world` (cache builder) share the same logic.
fn build_zone_graph(
    buildings: &[fellytip_shared::world::civilization::Building],
    _seed: u64,
) -> (ZoneRegistry, ZoneTopology) {
    use fellytip_shared::world::civilization::BuildingKind;
    use fellytip_shared::world::zone::ZoneId;

    let (mut registry, topology) = generate_zones(buildings, WORLD_SEED);

    let mut next_id: u32 = 1;
    for building in buildings {
        let floor_count = match building.kind {
            BuildingKind::Tavern | BuildingKind::Barracks => 2,
            BuildingKind::Tower => 4,
            BuildingKind::Keep => 3,
            _ => 0u8,
        };
        if floor_count < 2 { continue; }

        let floor_0_id = ZoneId(next_id);
        next_id += floor_count as u32;

        let world_x = building.tx as f32 - MAP_HALF_WIDTH  as f32 + 0.5;
        let world_y = building.ty as f32 - MAP_HALF_HEIGHT as f32 + 0.5;

        if let Some(zone) = registry.zones.get_mut(&floor_0_id) {
            for anchor in &mut zone.anchors {
                if anchor.name == "entrance" {
                    anchor.pos = Vec2::new(world_x, world_y);
                }
            }
        }
    }

    registry.zones.entry(OVERWORLD_ZONE).or_insert_with(|| Zone {
        id:          OVERWORLD_ZONE,
        kind:        ZoneKind::Overworld,
        parent:      ZoneParent::Overworld,
        world_id:    fellytip_shared::world::zone::WORLD_SURFACE,
        width:       1024,
        height:      1024,
        template_id: 0,
        anchors:     Vec::new(),
    });

    tracing::info!(
        zones   = registry.zones.len(),
        portals = topology.portals.len(),
        "Zone graph built"
    );

    (registry, topology)
}

/// Bevy startup system: build the zone graph and insert `ZoneRegistry` +
/// `ZoneTopology`.  Skips if `generate_world` already inserted cached resources.
pub fn populate_zones(
    mut commands: Commands,
    buildings: Option<Res<Buildings>>,
    existing: Option<Res<ZoneRegistry>>,
) {
    if existing.is_some() {
        tracing::info!("ZoneRegistry: using cached value");
        return;
    }

    let empty: Vec<fellytip_shared::world::civilization::Building> = Vec::new();
    let slice = buildings.as_deref().map(|b| b.0.as_slice()).unwrap_or(&empty);

    let (registry, topology) = build_zone_graph(slice, WORLD_SEED);
    commands.insert_resource(registry);
    commands.insert_resource(topology);
}

fn spawn_settlement_markers(settlements: Res<Settlements>, mut commands: Commands) {
    for settlement in &settlements.0 {
        commands.spawn((
            WorldPosition {
                x: settlement.x - MAP_HALF_WIDTH  as f32,
                y: settlement.y - MAP_HALF_HEIGHT as f32,
                z: settlement.z,
            },
            EntityKind::Settlement,
            settlement.kind,
        ));
        tracing::debug!(name = %settlement.name, "Settlement marker spawned");
    }
    tracing::info!(count = settlements.0.len(), "Settlement markers spawned");
}

fn history_warp(world: &mut World) {
    let ticks = world.resource::<MapGenConfig>().history_warp_ticks;
    if ticks == 0 {
        tracing::info!("History warp skipped (history_warp_ticks = 0)");
        return;
    }
    tracing::info!(ticks, "Starting history warp…");
    for _ in 0..ticks {
        world.resource_mut::<WorldSimTick>().0 += 1;
        world.run_schedule(crate::plugins::world_sim::WorldSimSchedule);
    }
    let tick = world.resource::<WorldSimTick>().0;
    tracing::info!(tick, "History warp complete — world is live");
}
