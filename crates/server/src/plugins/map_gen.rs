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
use smol_str::SmolStr;
use fellytip_shared::{
    WORLD_SEED,
    components::{EntityKind, WorldPosition},
    world::{
        civilization::{
            apply_building_tiles, generate_buildings, generate_roads,
            generate_settlements_full, Buildings, Settlements,
        },
        map::{generate_map, generate_spawn_points, WorldMap, MAP_HALF_WIDTH, MAP_HALF_HEIGHT},
        zone::{generate_zones, Portal, PortalKind, ZoneAnchor, ZoneKind, ZoneRegistry, ZoneTopology, OVERWORLD_ZONE},
        cave::find_portal_tiles,
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
const WORLD_CACHE_VERSION: u32 = 2;

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

    let (zone_registry, zone_topology) = build_zone_graph(&buildings, config.seed, Some(&map));
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

/// Generate the zone graph from buildings, then wire real world-space anchors
/// and Door portals connecting overworld ↔ each building's floor 0.
///
/// Kept as a standalone function so `populate_zones` (Bevy system) and
/// `generate_world` (cache builder) share the same logic.
///
/// `map` is used to read `CavePortal` tile positions stamped by
/// `generate_underground_civilization`; pass `None` to fall back to a
/// seeded position (used when no map is available at zone-graph build time).
fn build_zone_graph(
    buildings: &[fellytip_shared::world::civilization::Building],
    seed: u64,
    map: Option<&WorldMap>,
) -> (ZoneRegistry, ZoneTopology) {
    let (mut registry, mut topology, building_to_floor0) = generate_zones(buildings, seed);

    let mut next_portal_id = topology.portals.len() as u32;

    // Add a Door portal pair (overworld ↔ floor 0) for each multi-story building.
    for building in buildings {
        let Some(&floor_0_id) = building_to_floor0.get(&building.id) else { continue };

        let world_x = building.tx as f32 - MAP_HALF_WIDTH  as f32 + 0.5;
        let world_y = building.ty as f32 - MAP_HALF_HEIGHT as f32 + 0.5;
        let anchor_name = SmolStr::new(format!("door_{}", &building.id.to_string()[..8]));

        // Add the overworld-side anchor at the building's world-space position.
        if let Some(overworld) = registry.zones.get_mut(&OVERWORLD_ZONE) {
            overworld.anchors.push(ZoneAnchor {
                name: anchor_name.clone(),
                pos: Vec2::new(world_x, world_y),
            });
        }

        // Find floor-0's destination anchor (zone-local "entrance" point).
        let floor0_anchor = registry
            .zones
            .get(&floor_0_id)
            .and_then(|z| z.anchors.iter().find(|a| a.name == "entrance").map(|a| a.name.clone()))
            .unwrap_or_else(|| SmolStr::new("entrance"));

        // Overworld → floor 0.
        topology.add_portal(Portal {
            id: next_portal_id,
            kind: PortalKind::Door,
            from_zone: OVERWORLD_ZONE,
            from_anchor: anchor_name.clone(),
            trigger_radius: 1.0,
            traversal_cost: 1.0,
            faction_permeable: true,
            one_way: false,
            to_zone: floor_0_id,
            to_anchor: floor0_anchor.clone(),
            shape: None,
        });
        next_portal_id += 1;

        // Floor 0 → overworld (reverse).
        topology.add_portal(Portal {
            id: next_portal_id,
            kind: PortalKind::Door,
            from_zone: floor_0_id,
            from_anchor: floor0_anchor,
            trigger_radius: 1.0,
            traversal_cost: 1.0,
            faction_permeable: true,
            one_way: false,
            to_zone: OVERWORLD_ZONE,
            to_anchor: anchor_name,
            shape: None,
        });
        next_portal_id += 1;
    }

    // Wire CaveEntrance anchors from actual CavePortal tile positions stamped by
    // generate_underground_civilization. Falls back to a seeded position if the
    // map is unavailable or no portal tiles were placed.
    let underground_1_id = registry
        .zones
        .values()
        .find(|z| z.kind == ZoneKind::Underground { depth: 1 })
        .map(|z| z.id);

    let portal_tiles: Vec<(usize, usize)> = map
        .map(|m| find_portal_tiles(m, 1))
        .unwrap_or_default();

    if portal_tiles.is_empty() {
        // Seeded fallback (no map or no cave portals generated yet).
        let cave_x = (seed % 128) as f32 - 64.0;
        let cave_y = ((seed >> 8) % 128) as f32 - 64.0;
        if let Some(overworld) = registry.zones.get_mut(&OVERWORLD_ZONE) {
            overworld.anchors.push(ZoneAnchor {
                name: SmolStr::new("cave_entrance"),
                pos: Vec2::new(cave_x, cave_y),
            });
        }
    } else if let Some(u1_id) = underground_1_id {
        let underground_up = SmolStr::new("up");

        for (i, &(ix, iy)) in portal_tiles.iter().enumerate() {
            let world_x = ix as f32 - MAP_HALF_WIDTH  as f32 + 0.5;
            let world_y = iy as f32 - MAP_HALF_HEIGHT as f32 + 0.5;

            // First portal reuses the "cave_entrance" anchor already referenced by
            // the topology portal created in generate_zones. Additional portals
            // get unique anchor names + new portal pairs.
            let anchor_name = if i == 0 {
                SmolStr::new("cave_entrance")
            } else {
                SmolStr::new(format!("cave_entrance_{i}"))
            };

            if let Some(overworld) = registry.zones.get_mut(&OVERWORLD_ZONE) {
                overworld.anchors.push(ZoneAnchor {
                    name: anchor_name.clone(),
                    pos: Vec2::new(world_x, world_y),
                });
            }

            if i > 0 {
                topology.add_portal(Portal {
                    id: next_portal_id,
                    kind: PortalKind::CaveEntrance,
                    from_zone: OVERWORLD_ZONE,
                    from_anchor: anchor_name.clone(),
                    trigger_radius: 2.0,
                    traversal_cost: 2.0,
                    faction_permeable: true,
                    one_way: false,
                    to_zone: u1_id,
                    to_anchor: underground_up.clone(),
                    shape: None,
                });
                next_portal_id += 1;
                topology.add_portal(Portal {
                    id: next_portal_id,
                    kind: PortalKind::CaveEntrance,
                    from_zone: u1_id,
                    from_anchor: underground_up.clone(),
                    trigger_radius: 2.0,
                    traversal_cost: 2.0,
                    faction_permeable: true,
                    one_way: false,
                    to_zone: OVERWORLD_ZONE,
                    to_anchor: anchor_name,
                    shape: None,
                });
                next_portal_id += 1;
            }
        }
    }

    #[allow(unused_assignments)]
    { next_portal_id += 1; }

    tracing::info!(
        zones   = registry.zones.len(),
        portals = topology.portals.len(),
        cave_entrances = portal_tiles.len(),
        "Zone graph built"
    );

    (registry, topology)
}

/// Bevy startup system: build the zone graph and insert `ZoneRegistry` +
/// `ZoneTopology`.  Skips if `generate_world` already inserted cached resources.
pub fn populate_zones(
    mut commands: Commands,
    buildings: Option<Res<Buildings>>,
    map: Option<Res<WorldMap>>,
    existing: Option<Res<ZoneRegistry>>,
) {
    if existing.is_some() {
        tracing::info!("ZoneRegistry: using cached value");
        return;
    }

    let empty: Vec<fellytip_shared::world::civilization::Building> = Vec::new();
    let slice = buildings.as_deref().map(|b| b.0.as_slice()).unwrap_or(&empty);

    let (registry, topology) = build_zone_graph(slice, WORLD_SEED, map.as_deref());
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
