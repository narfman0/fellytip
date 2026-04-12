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

use bevy::prelude::*;
use fellytip_shared::{
    WORLD_SEED,
    world::map::{generate_map, MAP_HEIGHT, MAP_WIDTH},
};

use manager::{apply_chunk_meshes, rebuild_dirty_chunks, update_chunk_visibility,
              ChunkManager, TerrainAssets};
use material::create_terrain_material;

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_terrain_assets)
            .add_systems(
                Update,
                (update_chunk_visibility, rebuild_dirty_chunks, apply_chunk_meshes).chain(),
            );
    }
}

fn setup_terrain_assets(
    mut commands:  Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Regenerate the world map on the client — same deterministic call as the server.
    // Uses the default MAP_WIDTH/MAP_HEIGHT; a future WorldMeta replication will
    // propagate custom dimensions (same tech-debt note as the old tile_renderer).
    tracing::info!(seed = WORLD_SEED, "Client regenerating world map for terrain rendering…");
    let map = generate_map(WORLD_SEED, MAP_WIDTH, MAP_HEIGHT);
    tracing::info!("World map ready — setting up terrain chunks");

    let material = create_terrain_material(&mut materials);
    commands.insert_resource(TerrainAssets { material });
    commands.insert_resource(ChunkManager::default());
    commands.insert_resource(map);
}
