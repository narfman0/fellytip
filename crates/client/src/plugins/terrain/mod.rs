//! Smooth chunked terrain plugin.
//!
//! Replaces `TileRendererPlugin`. Divides the world map into 32×32-tile
//! chunks, each a single `Mesh` entity with smooth vertex-shared heights and
//! blended biome vertex colours.  Three LOD levels (Full / Half / Quarter)
//! are selected per chunk based on camera distance.

pub mod chunk;
pub mod lod;
pub mod manager;
pub mod material;

pub use manager::ChunkLifecycle;

use bevy::prelude::*;
use fellytip_shared::{
    WORLD_SEED,
    components::{Experience, WorldMeta},
    world::{
        civilization::{
            apply_building_tiles, generate_buildings, generate_settlements, Buildings, Settlements,
        },
        map::{generate_map, MAP_HEIGHT, MAP_WIDTH, WorldMap},
    },
};
use manager::{
    apply_chunk_meshes, rebuild_dirty_chunks, update_chunk_visibility,
    ChunkManager, TerrainAssets,
};
use material::create_terrain_material;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChunkLifecycle>()
            .add_systems(Startup, setup_terrain_assets)
            .add_systems(
                Update,
                (
                    apply_world_meta,
                    update_chunk_visibility,
                    rebuild_dirty_chunks,
                    apply_chunk_meshes,
                )
                    .chain(),
            );
    }
}

fn setup_terrain_assets(
    mut commands:  Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Regenerate the world map on the client with the default seed.  If the
    // server used a different seed it will send WorldMeta and apply_world_meta
    // will regenerate with the correct values.
    tracing::info!(seed = WORLD_SEED, "Client regenerating world map for terrain rendering…");
    let mut map = generate_map(WORLD_SEED, MAP_WIDTH, MAP_HEIGHT);
    tracing::info!("World map ready — setting up terrain chunks");

    let settlements = generate_settlements(&map, WORLD_SEED);
    let buildings = generate_buildings(&settlements, &map, WORLD_SEED);
    apply_building_tiles(&buildings, &mut map);

    let material = create_terrain_material(&mut materials);
    commands.insert_resource(TerrainAssets { material });
    commands.insert_resource(ChunkManager::default());
    commands.insert_resource(Settlements(settlements));
    commands.insert_resource(Buildings(buildings));
    commands.insert_resource(map);
}

// MULTIPLAYER: restore With<Replicated> to limit to server-sent player entities.
type ChangedWorldMeta = (With<Experience>, Changed<WorldMeta>);

/// When the server sends `WorldMeta` on the local player entity, regenerate the
/// `WorldMap` if the seed or dimensions differ.  This makes the client's terrain
/// identical to the server's, enabling client-authoritative movement prediction.
///
/// The query matches the replicated player entity (identified by `Experience`)
/// without needing `LocalPlayer` to be tagged yet, since `WorldMeta` may arrive
/// on the same frame as the first `Experience` replication.
fn apply_world_meta(
    query:        Query<&WorldMeta, ChangedWorldMeta>,
    mut map:      ResMut<WorldMap>,
    mut mgr:      ResMut<ChunkManager>,
    mut settlements: ResMut<Settlements>,
    mut buildings:   ResMut<Buildings>,
) {
    let Some(meta) = query.iter().next() else { return };

    if meta.seed        == map.seed
        && meta.width  as usize == map.width
        && meta.height as usize == map.height
    {
        return; // Already correct (common case: server using default seed).
    }

    tracing::info!(
        seed = meta.seed, width = meta.width, height = meta.height,
        "WorldMeta received — regenerating terrain to match server seed"
    );
    let mut new_map = generate_map(meta.seed, meta.width as usize, meta.height as usize);
    let new_settlements = generate_settlements(&new_map, meta.seed);
    let new_buildings = generate_buildings(&new_settlements, &new_map, meta.seed);
    apply_building_tiles(&new_buildings, &mut new_map);

    *map = new_map;
    *settlements = Settlements(new_settlements);
    *buildings = Buildings(new_buildings);

    // Reset chunk manager so all chunks are rebuilt from the new map data.
    mgr.lod_cache.clear();
    mgr.mesh_cache.clear();
    mgr.last_cam_chunk = None;
    // Spawned chunk entities will be despawned by apply_chunk_meshes on the
    // next frame (lod_cache is now empty, so all spawned are out-of-range).
}
