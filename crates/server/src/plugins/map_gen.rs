//! World generation and history pre-simulation.
//!
//! On startup:
//! 1. Generate the tile map from a fixed seed using fBm + biome + river passes.
//! 2. Place settlements.
//! 3. Assign territories and stamp the road network onto the map.
//! 4. Seed ecology from the world map biomes.
//! 5. Spawn faction guard NPCs at their home settlements.
//! 6. Run `WorldSimSchedule` for [`HISTORY_WARP_TICKS`] ticks at warp speed so
//!    factions and ecology have meaningful state before the first player connects.
//!
//! ## World map caching
//!
//! The full [`WorldMap`] (tiles + road flags) is written to a binary file
//! (`world_{seed}.bin`) on first generation.  Its path is recorded in the
//! `world_meta` table under the key `"world_map_file"`.  On subsequent startups
//! the file is read and deserialised, skipping the expensive fBm generation.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use fellytip_shared::{
    components::{EntityKind, WorldPosition},
    world::{
        civilization::{assign_territories, generate_roads, generate_settlements, Settlements},
        map::{generate_map, generate_spawn_points, WorldMap, MAP_HALF_WIDTH, MAP_HALF_HEIGHT},
    },
};

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

use crate::plugins::{
    ai::{flush_factions_to_db, init_population_state, seed_factions, spawn_faction_npcs},
    ecology::seed_ecology,
    persistence::Db,
    world_sim::WorldSimTick,
};

/// Key used in the `world_meta` table to store the world map file path.
const META_KEY_MAP_FILE: &str = "world_map_file";

pub struct MapGenPlugin;

impl Plugin for MapGenPlugin {
    fn build(&self, app: &mut App) {
        // apply_deferred is inserted between systems that use Commands and systems
        // that read the resources those commands insert, because Commands are
        // deferred in Bevy and are not flushed until the next apply_deferred.
        app.add_systems(
            Startup,
            (generate_world, ApplyDeferred, seed_ecology, spawn_faction_npcs, init_population_state, spawn_settlement_markers, history_warp, flush_factions_to_db)
                .chain()
                .after(seed_factions),
        );
    }
}

// ── DB helpers ────────────────────────────────────────────────────────────────

/// Read the cached map file path from `world_meta`, if present.
async fn get_map_file_path(pool: &sqlx::SqlitePool) -> Option<PathBuf> {
    let row: Option<(String,)> =
        sqlx::query_as::<_, (String,)>("SELECT value FROM world_meta WHERE key = ?")
            .bind(META_KEY_MAP_FILE)
            .fetch_optional(pool)
            .await
            .ok()?;
    row.map(|(path,)| PathBuf::from(path))
}

/// Persist the map file path into `world_meta` (upsert).
async fn set_map_file_path(pool: &sqlx::SqlitePool, path: &Path) {
    let res = sqlx::query(
        "INSERT OR REPLACE INTO world_meta (key, value) VALUES (?, ?)",
    )
    .bind(META_KEY_MAP_FILE)
    .bind(path.to_string_lossy().as_ref())
    .execute(pool)
    .await;

    if let Err(e) = res {
        tracing::warn!("Failed to write world_map_file to world_meta: {e}");
    }
}

// ── File helpers ──────────────────────────────────────────────────────────────

/// Try to load a [`WorldMap`] from a bincode file.  Returns `None` on any I/O
/// or deserialisation error so the caller can fall through to regeneration.
fn try_load_map_file(path: &PathBuf) -> Option<WorldMap> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(path = %path.display(), "Could not read world map file: {e}");
            return None;
        }
    };
    match bincode::deserialize::<WorldMap>(&bytes) {
        Ok(map) => Some(map),
        Err(e) => {
            tracing::warn!(path = %path.display(), "World map file corrupt: {e}");
            None
        }
    }
}

/// Serialise `map` to a bincode file named `world_{seed}_{width}x{height}.bin`.
/// Returns the path on success.
fn save_map_file(map: &WorldMap) -> Option<PathBuf> {
    let path = PathBuf::from(format!("world_{}_{}x{}.bin", map.seed, map.width, map.height));
    let bytes = match bincode::serialize(map) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Failed to serialise world map: {e}");
            return None;
        }
    };
    match std::fs::write(&path, &bytes) {
        Ok(()) => {
            tracing::info!(path = %path.display(), bytes = bytes.len(), "World map saved to file");
            Some(path)
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), "Failed to write world map file: {e}");
            None
        }
    }
}

// ── Generation helper ─────────────────────────────────────────────────────────

/// Run the full generation pipeline, save to file, and record the path in
/// `world_meta`.  Settlements are generated here only to stamp roads onto the
/// map; `generate_world` regenerates them afterwards for the ECS resource.
fn generate_and_save(
    pool: &sqlx::SqlitePool,
    rt: &tokio::runtime::Runtime,
    config: &MapGenConfig,
) -> WorldMap {
    tracing::info!(seed = config.seed, width = config.width, height = config.height, "Generating world map…");
    let mut map = generate_map(config.seed, config.width, config.height);
    let settlements = generate_settlements(&map, config.seed);
    generate_roads(&mut map, &settlements);
    let road_count = map.road_tiles.iter().filter(|&&r| r).count();
    tracing::info!(road_count, "Road network stamped");
    map.spawn_points = generate_spawn_points(&map);
    tracing::info!(count = map.spawn_points.len(), "Spawn points computed");

    if let Some(path) = save_map_file(&map) {
        rt.block_on(set_map_file_path(pool, &path));
    }

    map
}

// ── Bevy systems ──────────────────────────────────────────────────────────────

/// Generate (or load from cache) the world map + settlements and insert them
/// as Bevy resources.
fn generate_world(mut commands: Commands, db: Res<Db>, config: Res<MapGenConfig>) {
    let pool = db.pool().clone();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime for world map load");

    // Attempt to load the cached map file whose path is stored in world_meta.
    let map = match rt.block_on(get_map_file_path(&pool)) {
        Some(path) => {
            tracing::info!(path = %path.display(), "Found cached world map — attempting load");
            match try_load_map_file(&path) {
                Some(loaded)
                    if loaded.seed == config.seed
                        && loaded.width == config.width
                        && loaded.height == config.height
                        && !loaded.road_tiles.is_empty()
                        && !loaded.spawn_points.is_empty()
                        && loaded.columns.len() == config.width * config.height =>
                {
                    tracing::info!(seed = config.seed, "World map loaded from cache — skipping generation");
                    loaded
                }
                Some(_) => {
                    tracing::warn!(seed = config.seed, "Cached map failed validation — regenerating");
                    generate_and_save(&pool, &rt, &config)
                }
                None => generate_and_save(&pool, &rt, &config),
            }
        }
        None => {
            tracing::info!(seed = config.seed, "No cached world map — generating");
            generate_and_save(&pool, &rt, &config)
        }
    };

    let settlements = generate_settlements(&map, config.seed);
    tracing::info!(count = settlements.len(), "Settlements placed");

    let territory = assign_territories(&map, &settlements);
    let assigned = territory.iter().filter(|t| t.is_some()).count();
    tracing::info!(assigned, "Territory tiles assigned");

    commands.insert_resource(map);
    commands.insert_resource(Settlements(settlements));
    tracing::info!("World generation complete");
}

/// Spawn a static marker entity at each settlement so clients can render them.
///
/// Settlement markers carry `WorldPosition`, `EntityKind::Settlement`, and
/// `Replicate` — no `Health` or combat components.  They are never moved or
/// despawned during normal gameplay.
fn spawn_settlement_markers(settlements: Res<Settlements>, mut commands: Commands) {
    for settlement in &settlements.0 {
        commands.spawn((
            WorldPosition {
                x: settlement.x - MAP_HALF_WIDTH as f32,
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

/// Run `WorldSimSchedule` `config.history_warp_ticks` times synchronously before
/// players can connect.  This "ages" the world: factions expand, ecology
/// reaches equilibrium, and story events accumulate.
///
/// Set `history_warp_ticks = 0` in `server.local.toml` for fastest dev startup.
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
