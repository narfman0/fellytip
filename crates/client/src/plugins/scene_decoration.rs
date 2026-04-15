//! Scatter biome-appropriate 3D decorations (trees, rocks, vegetation) across
//! terrain chunks using Kenney Nature Kit GLB assets.
//!
//! # How it works
//! 1. `setup_decoration_assets` loads all nature GLBs at startup.
//! 2. Each frame, `apply_decorations` reads `ChunkLifecycle.newly_visible` and
//!    `newly_hidden`, spawning/despawning decorations accordingly.
//!    It drains both queues after processing.
//!
//! # Coordinate convention
//! Same as `terrain/chunk.rs`: tile `(gx, gy)` → Bevy `(gx − half_w, h, gy − half_h)`.

use std::collections::HashMap;

use bevy::prelude::*;
use fellytip_shared::world::map::{TileKind, WorldMap};

use super::terrain::chunk::{vertex_height, ChunkCoord};
use super::terrain::lod::CHUNK_TILES;
use super::terrain::ChunkLifecycle;

pub struct SceneDecorationPlugin;

impl Plugin for SceneDecorationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_decoration_assets)
            // Run after apply_chunk_meshes so newly_visible is already filled.
            .add_systems(Update, apply_decorations);
    }
}

// ── Resources ─────────────────────────────────────────────────────────────────

/// Scene handles grouped by decoration category.
#[derive(Resource)]
struct DecorationAssets {
    /// Broadleaf trees: oak, default, tall, detailed.
    broadleaf: Vec<Handle<Scene>>,
    /// Conifer trees: pine variants.
    conifer: Vec<Handle<Scene>>,
    /// Desert vegetation: cactus.
    desert: Vec<Handle<Scene>>,
    /// Tropical trees: palms.
    tropical: Vec<Handle<Scene>>,
    /// Rocks.
    rocks: Vec<Handle<Scene>>,
    /// Low shrubs / bushes.
    bushes: Vec<Handle<Scene>>,
}

/// Tracks decoration entities spawned per chunk for cleanup.
#[derive(Resource, Default)]
struct DecorationState {
    spawned: HashMap<ChunkCoord, Vec<Entity>>,
}

// ── Startup ───────────────────────────────────────────────────────────────────

fn setup_decoration_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    let load = |path: &str| -> Handle<Scene> {
        asset_server.load(format!("nature/{path}#Scene0"))
    };

    commands.insert_resource(DecorationAssets {
        broadleaf: vec![
            load("tree_default.glb"),
            load("tree_oak.glb"),
            load("tree_tall.glb"),
            load("tree_detailed.glb"),
        ],
        conifer: vec![
            load("tree_pineDefaultA.glb"),
            load("tree_pineDefaultB.glb"),
            load("tree_pineTallA.glb"),
        ],
        desert: vec![
            load("cactus_tall.glb"),
            load("cactus_short.glb"),
        ],
        tropical: vec![
            load("tree_palm.glb"),
            load("tree_palmTall.glb"),
        ],
        rocks: vec![
            load("rock_largeA.glb"),
            load("rock_largeB.glb"),
            load("rock_largeC.glb"),
            load("rock_tallA.glb"),
            load("rock_tallB.glb"),
            load("rock_tallC.glb"),
        ],
        bushes: vec![
            load("plant_bush.glb"),
            load("plant_bushLarge.glb"),
            load("grass.glb"),
            load("grass_large.glb"),
        ],
    });
    commands.init_resource::<DecorationState>();
}

// ── Systems ───────────────────────────────────────────────────────────────────

fn apply_decorations(
    mut commands: Commands,
    mut state: ResMut<DecorationState>,
    mut lifecycle: ResMut<ChunkLifecycle>,
    assets: Res<DecorationAssets>,
    map: Res<WorldMap>,
) {
    // ── Despawn decorations for hidden chunks ──────────────────────────────────

    for (coord, _entity) in lifecycle.newly_hidden.iter() {
        if let Some(entities) = state.spawned.remove(coord) {
            for ent in entities {
                commands.entity(ent).despawn();
            }
        }
    }

    // ── Spawn decorations for newly visible chunks ─────────────────────────────

    let half_w = (map.width  / 2) as i32;
    let half_h = (map.height / 2) as i32;

    for (coord, _entity) in lifecycle.newly_visible.iter().copied() {
        if state.spawned.contains_key(&coord) {
            continue; // already decorated (LOD re-emit on same chunk)
        }

        let mut chunk_entities: Vec<Entity> = Vec::new();

        let base_x = coord.cx * CHUNK_TILES as i32;
        let base_y = coord.cy * CHUNK_TILES as i32;

        for dy in 0..CHUNK_TILES as i32 {
            for dx in 0..CHUNK_TILES as i32 {
                let gx = (base_x + dx).clamp(0, map.width  as i32 - 1) as usize;
                let gy = (base_y + dy).clamp(0, map.height as i32 - 1) as usize;

                let kind = map.column(gx, gy).layers
                    .iter().rev()
                    .find(|l| l.is_surface_kind())
                    .or_else(|| map.column(gx, gy).layers.last())
                    .map(|l| l.kind)
                    .unwrap_or(TileKind::Void);

                let h = tile_hash(map.seed, gx, gy);

                let Some((scene_list, density, scale_base)) =
                    decoration_for_biome(kind, &assets) else { continue };

                // density is out of 256; lower = more decorations.
                if (h & 0xFF) as u32 >= density {
                    continue;
                }

                let idx       = ((h >>  8) as usize) % scene_list.len();
                let yaw       = (((h >> 16) & 0xFF) as f32 / 255.0) * std::f32::consts::TAU;
                let scale_var = 0.9 + (((h >> 24) & 0xFF) as f32 / 255.0) * 0.2; // ±10%
                let scale     = scale_base * scale_var;

                let bx = gx as f32 - half_w as f32;
                let bz = gy as f32 - half_h as f32;
                let by = vertex_height(&map, gx, gy);

                let transform = Transform::from_xyz(bx, by, bz)
                    .with_rotation(Quat::from_rotation_y(yaw))
                    .with_scale(Vec3::splat(scale));

                let ent = commands
                    .spawn((SceneRoot(scene_list[idx].clone()), transform))
                    .id();
                chunk_entities.push(ent);
            }
        }

        state.spawned.insert(coord, chunk_entities);
    }

    // Drain the lifecycle queues — they are refilled next frame.
    lifecycle.newly_visible.clear();
    lifecycle.newly_hidden.clear();
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns `(scene_list, density_threshold_out_of_256, scale)` for the biome,
/// or `None` if no decoration should appear (water, void, river).
fn decoration_for_biome(
    kind: TileKind,
    assets: &DecorationAssets,
) -> Option<(&Vec<Handle<Scene>>, u32, f32)> {
    match kind {
        TileKind::Forest | TileKind::TemperateForest =>
            Some((&assets.broadleaf, 64, 1.0)),         // ~25% tiles
        TileKind::TropicalForest | TileKind::TropicalRainforest =>
            Some((&assets.tropical, 77, 1.2)),           // ~30%
        TileKind::Taiga =>
            Some((&assets.conifer, 64, 1.0)),            // ~25%
        TileKind::Mountain | TileKind::Stone =>
            Some((&assets.rocks, 38, 0.8)),              // ~15%
        TileKind::Tundra | TileKind::Arctic | TileKind::PolarDesert =>
            Some((&assets.rocks, 20, 0.6)),              // ~8%
        TileKind::Desert =>
            Some((&assets.desert, 20, 1.0)),             // ~8%
        TileKind::Savanna =>
            Some((&assets.bushes, 20, 0.7)),             // ~8%
        TileKind::Plains | TileKind::Grassland =>
            Some((&assets.bushes, 13, 0.6)),             // ~5%
        // No decorations on water, rivers, or void.
        TileKind::Water | TileKind::River | TileKind::Void => None,
    }
}

/// Deterministic tile hash seeded by world seed + tile position.
fn tile_hash(seed: u64, gx: usize, gy: usize) -> u64 {
    let v = seed
        .wrapping_add((gx as u64).wrapping_mul(2654435761))
        .wrapping_add((gy as u64).wrapping_mul(805459861));
    // avalanche mixing
    let v = v ^ (v >> 33);
    let v = v.wrapping_mul(0xff51afd7ed558ccd);
    let v = v ^ (v >> 33);
    let v = v.wrapping_mul(0xc4ceb9fe1a85ec53);
    v ^ (v >> 33)
}
