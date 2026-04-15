//! `ChunkManager` resource and the three systems that keep terrain chunks in
//! sync with the camera:
//!
//! 1. `update_chunk_visibility` — recomputes which chunks are visible and at
//!    what LOD when the camera moves to a new chunk.
//! 2. `rebuild_dirty_chunks` — builds or rebuilds `Mesh` assets for every
//!    chunk that changed LOD or entered the view this frame.
//! 3. `apply_chunk_meshes` — spawns/despawns Bevy entities and swaps mesh
//!    handles when LOD changes, keeping ECS in sync with `ChunkManager`.

use std::collections::{HashMap, HashSet, VecDeque};

use bevy::prelude::*;
use fellytip_shared::world::map::WorldMap;

use super::chunk::{build_chunk_mesh, ChunkCoord};
use super::lod::{EdgeTransitions, LodLevel, CHUNK_TILES};
use super::water::build_water_mesh;
use super::water_material::{WaterAssets, WaterMaterial};
use crate::plugins::camera::OrbitCamera;

// ── Chunk lifecycle notifications ─────────────────────────────────────────────

/// Per-frame lists of chunks that just became visible or hidden.
///
/// Decoration and other systems drain these each frame to react to chunk
/// lifecycle without needing Bevy events.  `apply_chunk_meshes` fills them;
/// consumer systems should drain them (via `clear()`) after processing.
#[derive(Resource, Default)]
pub struct ChunkLifecycle {
    /// Chunks that first became visible this frame (coord + mesh entity).
    pub newly_visible: Vec<(ChunkCoord, Entity)>,
    /// Chunks that were hidden this frame (coord + mesh entity).
    pub newly_hidden: Vec<(ChunkCoord, Entity)>,
}

// ── Resource ──────────────────────────────────────────────────────────────────

/// State for the chunk terrain system.
#[derive(Resource)]
pub struct ChunkManager {
    /// Entities currently representing visible chunks.
    pub spawned: HashMap<ChunkCoord, Entity>,
    /// Most-recent LOD assigned to each visible chunk.
    pub lod_cache: HashMap<ChunkCoord, LodLevel>,
    /// Cached mesh handles keyed by (coord, lod) to avoid rebuilding unchanged meshes.
    pub mesh_cache: HashMap<(ChunkCoord, LodLevel), Handle<Mesh>>,
    /// Chunks whose terrain mesh must be (re)built this frame.
    pub dirty: HashSet<ChunkCoord>,
    /// Camera chunk from the previous frame — skip work when camera hasn't moved.
    pub last_cam_chunk: Option<ChunkCoord>,
    /// View radius in chunks.  20 chunks × 32 tiles = 640 tiles, which exceeds
    /// the camera's max zoom distance of 400 world units with room for Eighth LOD.
    pub render_radius: i32,

    // ── Water overlay (parallel to terrain) ──────────────────────────────────

    /// Water entities currently spawned (one per chunk that has water tiles).
    pub water_spawned: HashMap<ChunkCoord, Entity>,
    /// Cached water mesh handles keyed by (coord, lod).
    pub water_mesh_cache: HashMap<(ChunkCoord, LodLevel), Handle<Mesh>>,
    /// Chunks whose water mesh must be (re)built this frame.
    pub water_dirty: HashSet<ChunkCoord>,
}

impl Default for ChunkManager {
    fn default() -> Self {
        Self {
            spawned:          HashMap::new(),
            lod_cache:        HashMap::new(),
            mesh_cache:       HashMap::new(),
            dirty:            HashSet::new(),
            last_cam_chunk:   None,
            render_radius:    20,
            water_spawned:    HashMap::new(),
            water_mesh_cache: HashMap::new(),
            water_dirty:      HashSet::new(),
        }
    }
}

// ── TerrainAssets resource ────────────────────────────────────────────────────

/// Shared GPU handles inserted at startup.
#[derive(Resource)]
pub struct TerrainAssets {
    pub material: Handle<StandardMaterial>,
}

// ── System 1: visibility + LOD selection ─────────────────────────────────────

pub fn update_chunk_visibility(
    camera_q: Query<&OrbitCamera>,
    map:      Res<WorldMap>,
    mut mgr:  ResMut<ChunkManager>,
) {
    let Ok(cam) = camera_q.single() else { return };

    // Camera target in Bevy space (X east, Y up, Z south) → tile grid position.
    let target = cam.target;
    let half_w = (map.width  / 2) as f32;
    let half_h = (map.height / 2) as f32;
    let tile_x = (target.x + half_w) as i32;
    let tile_z = (target.z + half_h) as i32;
    let cam_chunk = ChunkCoord::from_tile(tile_x, tile_z);

    // Skip rebuild when camera hasn't moved to a new chunk.
    if mgr.last_cam_chunk == Some(cam_chunk) {
        return;
    }
    mgr.last_cam_chunk = Some(cam_chunk);

    let r = mgr.render_radius;
    let cam_world = Vec3::new(target.x, target.y, target.z);

    // ── Assign initial LOD for each visible chunk ─────────────────────────────

    let mut new_lod: HashMap<ChunkCoord, LodLevel> = HashMap::new();
    let mut visible: HashSet<ChunkCoord>            = HashSet::new();

    for dy in -r..=r {
        for dx in -r..=r {
            let coord = ChunkCoord {
                cx: cam_chunk.cx + dx,
                cy: cam_chunk.cy + dy,
            };
            // Skip chunks fully outside the map.
            let n_chunks_x = map.width.div_ceil(CHUNK_TILES) as i32;
            let n_chunks_y = map.height.div_ceil(CHUNK_TILES) as i32;
            if coord.cx < 0 || coord.cy < 0
                || coord.cx >= n_chunks_x
                || coord.cy >= n_chunks_y
            {
                continue;
            }
            let dist = cam_world.distance(coord.world_center(&map));
            new_lod.insert(coord, LodLevel::from_distance(dist));
            visible.insert(coord);
        }
    }

    // ── LOD clamping BFS (|lod_a − lod_b| ≤ 1 for neighbours) ───────────────

    let mut queue: VecDeque<ChunkCoord> = visible.iter().copied().collect();
    while let Some(coord) = queue.pop_front() {
        let my_lod = new_lod[&coord];
        for (ddx, ddy) in [(1i32,0),(-1,0),(0,1),(0,-1)] {
            let nb = ChunkCoord { cx: coord.cx + ddx, cy: coord.cy + ddy };
            if let Some(nb_lod) = new_lod.get_mut(&nb) {
                if *nb_lod > my_lod.coarser() {
                    *nb_lod = my_lod.coarser();
                    queue.push_back(nb);
                }
            }
        }
    }

    // ── Mark dirty ────────────────────────────────────────────────────────────

    for (&coord, &lod) in &new_lod {
        let changed = mgr.lod_cache.get(&coord) != Some(&lod);
        if changed {
            mgr.dirty.insert(coord);
            mgr.water_dirty.insert(coord);
        }
    }
    // Newly visible chunks not yet in lod_cache.
    for &coord in &visible {
        if !mgr.lod_cache.contains_key(&coord) {
            mgr.dirty.insert(coord);
            mgr.water_dirty.insert(coord);
        }
    }

    // Out-of-range chunks will be cleaned up in apply_chunk_meshes.
    mgr.lod_cache.retain(|k, _| visible.contains(k));
    for (coord, lod) in new_lod {
        mgr.lod_cache.insert(coord, lod);
    }
}

// ── System 2: mesh rebuild ────────────────────────────────────────────────────

pub fn rebuild_dirty_chunks(
    map:       Res<WorldMap>,
    mut mgr:   ResMut<ChunkManager>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if mgr.dirty.is_empty() {
        return;
    }

    let dirty: Vec<ChunkCoord> = mgr.dirty.drain().collect();

    for coord in dirty {
        let Some(&lod) = mgr.lod_cache.get(&coord) else { continue };

        // Compute edge-transition flags from neighbour LODs.
        let transitions = EdgeTransitions {
            north: is_coarser_neighbor(&mgr.lod_cache, coord,  0, -1, lod),
            south: is_coarser_neighbor(&mgr.lod_cache, coord,  0,  1, lod),
            west:  is_coarser_neighbor(&mgr.lod_cache, coord, -1,  0, lod),
            east:  is_coarser_neighbor(&mgr.lod_cache, coord,  1,  0, lod),
        };
        let mesh = build_chunk_mesh(&map, coord, lod, transitions);
        let handle = meshes.add(mesh);
        mgr.mesh_cache.insert((coord, lod), handle);
    }
}

fn is_coarser_neighbor(
    lod_cache: &HashMap<ChunkCoord, LodLevel>,
    coord: ChunkCoord,
    dx: i32,
    dy: i32,
    my_lod: LodLevel,
) -> bool {
    let nb = ChunkCoord { cx: coord.cx + dx, cy: coord.cy + dy };
    lod_cache.get(&nb).is_some_and(|&nb_lod| nb_lod > my_lod)
}

// ── System 3: ECS sync ────────────────────────────────────────────────────────

pub fn apply_chunk_meshes(
    mut commands:  Commands,
    mut mgr:       ResMut<ChunkManager>,
    assets:        Res<TerrainAssets>,
    mut lifecycle: ResMut<ChunkLifecycle>,
) {
    // ── Despawn chunks no longer in lod_cache ─────────────────────────────────

    let visible: HashSet<ChunkCoord> = mgr.lod_cache.keys().copied().collect();
    let to_despawn: Vec<ChunkCoord> = mgr.spawned.keys()
        .filter(|k| !visible.contains(k))
        .copied()
        .collect();

    for coord in to_despawn {
        if let Some(entity) = mgr.spawned.remove(&coord) {
            lifecycle.newly_hidden.push((coord, entity));
            commands.entity(entity).despawn();
        }
    }

    // ── Spawn or update visible chunks ────────────────────────────────────────

    let to_update: Vec<(ChunkCoord, LodLevel)> = mgr.lod_cache
        .iter()
        .map(|(&c, &l)| (c, l))
        .collect();

    for (coord, lod) in to_update {
        let Some(handle) = mgr.mesh_cache.get(&(coord, lod)).cloned() else { continue };

        if let Some(&entity) = mgr.spawned.get(&coord) {
            // Entity already exists — just update its mesh handle.
            commands.entity(entity).insert(Mesh3d(handle));
        } else {
            // Spawn a new chunk entity. Vertex positions are in world space,
            // so Transform::IDENTITY places it correctly with no offset.
            let entity = commands.spawn((
                Mesh3d(handle),
                MeshMaterial3d(assets.material.clone()),
                Transform::IDENTITY,
            )).id();
            mgr.spawned.insert(coord, entity);
            lifecycle.newly_visible.push((coord, entity));
        }
    }
}

// ── System 4: water mesh rebuild ─────────────────────────────────────────────

/// Build or rebuild flat water meshes for chunks in `water_dirty`.
///
/// Chunks with no water tiles produce `None` from `build_water_mesh`; those are
/// skipped and no handle is cached, so `apply_water_meshes` will not spawn an
/// entity for them.
pub fn rebuild_dirty_water(
    map:        Res<WorldMap>,
    mut mgr:    ResMut<ChunkManager>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if mgr.water_dirty.is_empty() {
        return;
    }

    let dirty: Vec<ChunkCoord> = mgr.water_dirty.drain().collect();

    for coord in dirty {
        let Some(&lod) = mgr.lod_cache.get(&coord) else { continue };

        if let Some(mesh) = build_water_mesh(&map, coord, lod) {
            let handle = meshes.add(mesh);
            mgr.water_mesh_cache.insert((coord, lod), handle);
        }
        // When build_water_mesh returns None the chunk has no water tiles.
        // No handle is inserted; apply_water_meshes will not spawn an entity.
    }
}

// ── System 5: water ECS sync ──────────────────────────────────────────────────

/// Spawn, update, or despawn water overlay entities to match `lod_cache`.
///
/// Mirrors `apply_chunk_meshes` but for water.  No `ChunkLifecycle`
/// notifications are emitted — decorations only track terrain, not water.
pub fn apply_water_meshes(
    mut commands: Commands,
    mut mgr:      ResMut<ChunkManager>,
    assets:       Res<WaterAssets>,
) {
    // ── Despawn water entities for chunks no longer visible ───────────────────

    let visible: HashSet<ChunkCoord> = mgr.lod_cache.keys().copied().collect();
    let to_despawn: Vec<ChunkCoord> = mgr.water_spawned.keys()
        .filter(|k| !visible.contains(k))
        .copied()
        .collect();

    for coord in to_despawn {
        if let Some(entity) = mgr.water_spawned.remove(&coord) {
            commands.entity(entity).despawn();
        }
    }

    // ── Spawn or update visible water chunks ──────────────────────────────────

    let to_update: Vec<(ChunkCoord, LodLevel)> = mgr.lod_cache
        .iter()
        .map(|(&c, &l)| (c, l))
        .collect();

    for (coord, lod) in to_update {
        let Some(handle) = mgr.water_mesh_cache.get(&(coord, lod)).cloned() else {
            // Chunk is land-only (no water mesh cached) — skip.
            continue;
        };

        if let Some(&entity) = mgr.water_spawned.get(&coord) {
            commands.entity(entity).insert(Mesh3d(handle));
        } else {
            let entity = commands.spawn((
                Mesh3d(handle),
                MeshMaterial3d(assets.material.clone()),
                Transform::IDENTITY,
            )).id();
            mgr.water_spawned.insert(coord, entity);
        }
    }
}

// ── System 6: animate water ───────────────────────────────────────────────────

/// Write elapsed time into the shared `WaterMaterial` uniform each frame.
///
/// All water entities share one material handle, so a single mutation here
/// drives animation across every visible water chunk simultaneously.
pub fn update_water_time(
    time:      Res<Time>,
    assets:    Res<WaterAssets>,
    mut mats:  ResMut<Assets<WaterMaterial>>,
) {
    if let Some(mat) = mats.get_mut(&assets.material) {
        mat.extension.time = time.elapsed_secs();
    }
}
