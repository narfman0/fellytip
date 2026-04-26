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

use super::chunk::{build_cave_chunk_mesh, build_chunk_mesh, ChunkCoord};
use super::lod::{EdgeTransitions, LodLevel, CHUNK_TILES};
use crate::plugins::camera::OrbitCamera;
use crate::{LocalPlayer, PredictedPosition};

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
    /// Chunks whose mesh must be (re)built this frame.
    pub dirty: HashSet<ChunkCoord>,
    /// Camera chunk from the previous frame — skip work when camera hasn't moved.
    pub last_cam_chunk: Option<ChunkCoord>,
    /// View radius in chunks.  20 chunks × 32 tiles = 640 tiles, which exceeds
    /// the camera's max zoom distance of 400 world units with room for Eighth LOD.
    pub render_radius: i32,
}

impl Default for ChunkManager {
    fn default() -> Self {
        Self {
            spawned:        HashMap::new(),
            lod_cache:      HashMap::new(),
            mesh_cache:     HashMap::new(),
            dirty:          HashSet::new(),
            last_cam_chunk: None,
            render_radius:  20,
        }
    }
}

// ── TerrainAssets resource ────────────────────────────────────────────────────

/// Shared GPU handles inserted at startup.
#[derive(Resource)]
pub struct TerrainAssets {
    pub material: Handle<StandardMaterial>,
}

/// Marker on surface terrain chunk entities (z >= 0).
#[derive(Component)]
pub struct SurfaceTerrain;

/// Marker on cave terrain chunk entities (z < 0).
#[derive(Component)]
pub struct CaveTerrain;

/// State for the cave chunk terrain system.
#[derive(Resource, Default)]
pub struct CaveChunkManager {
    pub spawned: HashMap<ChunkCoord, Entity>,
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
        }
    }
    // Newly visible chunks not yet in lod_cache.
    for &coord in &visible {
        if !mgr.lod_cache.contains_key(&coord) {
            mgr.dirty.insert(coord);
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
                SurfaceTerrain,
            )).id();
            mgr.spawned.insert(coord, entity);
            lifecycle.newly_visible.push((coord, entity));
        }
    }
}

// ── Cave chunk system ─────────────────────────────────────────────────────────

use fellytip_shared::world::cave::cave_z;

/// Spawn cave chunk mesh entities on first call (triggered by WorldMap resource).
/// Cave depth 1 lives at `cave_z(1) = -10.0`.
pub fn spawn_cave_chunks(
    mut commands: Commands,
    map: Res<WorldMap>,
    assets: Res<TerrainAssets>,
    mut cave_mgr: ResMut<CaveChunkManager>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if !cave_mgr.spawned.is_empty() { return; }
    if !map.is_changed() && !map.is_added() { return; }

    let z = cave_z(1);
    let n_cx = map.width.div_ceil(CHUNK_TILES) as i32;
    let n_cy = map.height.div_ceil(CHUNK_TILES) as i32;

    for cy in 0..n_cy {
        for cx in 0..n_cx {
            let coord = ChunkCoord { cx, cy };
            let mesh = build_cave_chunk_mesh(&map, coord, z);
            let handle = meshes.add(mesh);
            let entity = commands.spawn((
                Mesh3d(handle),
                MeshMaterial3d(assets.material.clone()),
                Transform::IDENTITY,
                CaveTerrain,
                Visibility::Hidden,
            )).id();
            cave_mgr.spawned.insert(coord, entity);
        }
    }
}

/// Show/hide surface and cave terrain layers based on the local player's z.
///
/// Surface layer (z >= -1.0): show surface chunks, hide cave chunks.
/// Underground (z < -1.0): hide surface chunks, show cave chunks.
pub fn update_layer_visibility(
    player_q: Query<&PredictedPosition, With<LocalPlayer>>,
    mut surface_q: Query<&mut Visibility, (With<SurfaceTerrain>, Without<CaveTerrain>)>,
    mut cave_q: Query<&mut Visibility, (With<CaveTerrain>, Without<SurfaceTerrain>)>,
) {
    let Ok(pos) = player_q.single() else { return };
    let underground = pos.z < -1.0;
    let surface_vis = if underground { Visibility::Hidden } else { Visibility::Visible };
    let cave_vis    = if underground { Visibility::Visible } else { Visibility::Hidden };
    for mut v in &mut surface_q {
        if *v != surface_vis { *v = surface_vis; }
    }
    for mut v in &mut cave_q {
        if *v != cave_vis { *v = cave_vis; }
    }
}
