//! Scatter biome-appropriate 3D decorations (trees, rocks, vegetation) across
//! terrain chunks using Kenney Nature Kit GLB assets.
//!
//! # Performance design
//!
//! The terrain renderer streams up to 1,600 chunks (radius 20) but decorations
//! are only placed within `DECORATION_RADIUS = 6` chunks of the camera — roughly
//! 144 chunks max vs 1,600.  New chunks are queued and processed at most
//! `CHUNKS_PER_FRAME = 2` per frame to eliminate startup spikes.  Each chunk
//! is capped at `MAX_PER_CHUNK = 16` decoration entities regardless of biome
//! density, bounding worst-case entity count to ~2,300 total.
//!
//! # Coordinate convention
//! Same as `terrain/chunk.rs`: tile `(gx, gy)` → Bevy `(gx − half_w, h, gy − half_h)`.

use std::collections::{HashMap, VecDeque};

use bevy::prelude::*;
use fellytip_shared::world::map::{TileKind, WorldMap};

use super::terrain::chunk::{vertex_height, ChunkCoord};
use super::terrain::lod::CHUNK_TILES;
use super::terrain::{ChunkLifecycle, manager::ChunkManager};

pub struct SceneDecorationPlugin;

// ── Tuning constants ──────────────────────────────────────────────────────────

/// Chebyshev chunk distance within which decorations are placed.
/// Smaller = fewer entities; larger = denser coverage further out.
const DECORATION_RADIUS: i32 = 6;

/// Maximum decoration entities per chunk.  Caps worst-case entity count.
const MAX_PER_CHUNK: usize = 16;

/// Chunks processed per frame from the pending queue.  Spreads startup cost
/// over many frames so the first frame never spikes.
const CHUNKS_PER_FRAME: usize = 2;

impl Plugin for SceneDecorationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_decoration_assets)
            // Run after apply_chunk_meshes so newly_visible/hidden are filled.
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

/// Per-chunk decoration state + pending spawn queue.
#[derive(Resource, Default)]
struct DecorationState {
    /// Decoration entities already spawned, keyed by chunk coord.
    spawned: HashMap<ChunkCoord, Vec<Entity>>,
    /// Chunks queued for decoration but not yet processed.
    /// Processed at most `CHUNKS_PER_FRAME` per frame to smooth startup cost.
    pending: VecDeque<ChunkCoord>,
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
    mgr: Res<ChunkManager>,
) {
    // ── Despawn decorations for hidden chunks (always immediate) ───────────────

    for (coord, _entity) in lifecycle.newly_hidden.iter() {
        if let Some(entities) = state.spawned.remove(coord) {
            for ent in entities {
                commands.entity(ent).despawn();
            }
        }
        // Also remove from pending if it somehow queued but never processed.
        state.pending.retain(|c| c != coord);
    }

    // ── Queue newly visible chunks (filtered by decoration radius) ─────────────

    let cam_chunk = mgr.last_cam_chunk;

    for (coord, _entity) in lifecycle.newly_visible.iter().copied() {
        // Skip if already decorated or already queued.
        if state.spawned.contains_key(&coord) || state.pending.contains(&coord) {
            continue;
        }
        // Only decorate chunks within DECORATION_RADIUS of camera.
        if let Some(cc) = cam_chunk {
            let dist = (coord.cx - cc.cx).abs().max((coord.cy - cc.cy).abs());
            if dist > DECORATION_RADIUS {
                continue;
            }
        }
        state.pending.push_back(coord);
    }

    // Drain lifecycle — refilled next frame.
    lifecycle.newly_visible.clear();
    lifecycle.newly_hidden.clear();

    // ── Process pending chunks (rate-limited to CHUNKS_PER_FRAME) ─────────────

    let half_w = (map.width  / 2) as i32;
    let half_h = (map.height / 2) as i32;

    for _ in 0..CHUNKS_PER_FRAME {
        let Some(coord) = state.pending.pop_front() else { break };

        // Skip if already decorated (e.g. enqueued twice) or no longer visible.
        if state.spawned.contains_key(&coord) {
            continue;
        }

        // Re-check radius: camera may have moved since this chunk was enqueued.
        if let Some(cc) = cam_chunk {
            let dist = (coord.cx - cc.cx).abs().max((coord.cy - cc.cy).abs());
            if dist > DECORATION_RADIUS {
                continue; // Drop without decorating — too far now.
            }
        }

        let mut chunk_entities: Vec<Entity> = Vec::new();

        let base_x = coord.cx * CHUNK_TILES as i32;
        let base_y = coord.cy * CHUNK_TILES as i32;

        'tile: for dy in 0..CHUNK_TILES as i32 {
            for dx in 0..CHUNK_TILES as i32 {
                if chunk_entities.len() >= MAX_PER_CHUNK {
                    break 'tile;
                }

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

                // density out of 256 — lower value = more sparse.
                if (h & 0xFF) as u32 >= density {
                    continue;
                }

                let idx       = ((h >>  8) as usize) % scene_list.len();
                let yaw       = (((h >> 16) & 0xFF) as f32 / 255.0) * std::f32::consts::TAU;
                let scale_var = 0.9 + (((h >> 24) & 0xFF) as f32 / 255.0) * 0.2;
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
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns `(scene_list, density_threshold_out_of_256, scale)` for the biome,
/// or `None` if no decoration should appear (water, void, river).
///
/// Densities are kept low (~5–12%) to limit entity count.
fn decoration_for_biome(
    kind: TileKind,
    assets: &DecorationAssets,
) -> Option<(&Vec<Handle<Scene>>, u32, f32)> {
    match kind {
        TileKind::Forest | TileKind::TemperateForest =>
            Some((&assets.broadleaf, 26, 1.0)),    // ~10%
        TileKind::TropicalForest | TileKind::TropicalRainforest =>
            Some((&assets.tropical, 32, 1.2)),     // ~12%
        TileKind::Taiga =>
            Some((&assets.conifer, 26, 1.0)),      // ~10%
        TileKind::Mountain | TileKind::Stone =>
            Some((&assets.rocks, 19, 0.8)),        // ~7%
        TileKind::Tundra | TileKind::Arctic | TileKind::PolarDesert =>
            Some((&assets.rocks, 13, 0.6)),        // ~5%
        TileKind::Desert =>
            Some((&assets.desert, 13, 1.0)),       // ~5%
        TileKind::Savanna =>
            Some((&assets.bushes, 13, 0.7)),       // ~5%
        TileKind::Plains | TileKind::Grassland =>
            Some((&assets.bushes, 10, 0.6)),       // ~4%
        TileKind::Water | TileKind::River | TileKind::Void => None,
    }
}

/// Deterministic tile hash seeded by world seed + tile position.
fn tile_hash(seed: u64, gx: usize, gy: usize) -> u64 {
    let v = seed
        .wrapping_add((gx as u64).wrapping_mul(2654435761))
        .wrapping_add((gy as u64).wrapping_mul(805459861));
    let v = v ^ (v >> 33);
    let v = v.wrapping_mul(0xff51afd7ed558ccd);
    let v = v ^ (v >> 33);
    let v = v.wrapping_mul(0xc4ceb9fe1a85ec53);
    v ^ (v >> 33)
}
