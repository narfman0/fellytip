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
use fellytip_shared::world::map::{TileKind, WorldMap};

use super::chunk::{build_chunk_mesh, build_underground_chunk_mesh, ChunkCoord};
use super::lod::{EdgeTransitions, LodLevel, CHUNK_TILES};
use crate::plugins::camera::OrbitCamera;

// ── Active layer ──────────────────────────────────────────────────────────────

/// Which rendering layer is currently active, determined by the player's depth.
///
/// `OrbitCamera.target.y` equals `PredictedPosition.z` (player elevation in
/// Bevy Y-up coordinates).  Thresholds are midpoints between tier floors:
/// surface ≈ 0, Cavern = −15, DeepRock = −38, LuminousGrotto = −65.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ActiveLayer {
    #[default]
    Surface,
    Cave(TileKind),
}

impl ActiveLayer {
    pub fn from_camera_y(y: f32) -> Self {
        if      y > -7.5  { Self::Surface }
        else if y > -26.5 { Self::Cave(TileKind::Cavern) }
        else if y > -51.5 { Self::Cave(TileKind::DeepRock) }
        else              { Self::Cave(TileKind::LuminousGrotto) }
    }
}

// ── Resource ──────────────────────────────────────────────────────────────────

/// State for the chunk terrain system.
#[derive(Resource)]
pub struct ChunkManager {
    /// Entities currently representing visible chunks.
    pub spawned: HashMap<ChunkCoord, Entity>,
    /// Most-recent LOD assigned to each visible chunk.
    pub lod_cache: HashMap<ChunkCoord, LodLevel>,
    /// Cached mesh handles keyed by (coord, lod, layer) to avoid rebuilding unchanged meshes.
    /// Underground meshes always use `LodLevel::Full` as the LOD key (flat floors need no LOD).
    pub mesh_cache: HashMap<(ChunkCoord, LodLevel, ActiveLayer), Handle<Mesh>>,
    /// Chunks whose mesh must be (re)built this frame.
    pub dirty: HashSet<ChunkCoord>,
    /// Camera chunk from the previous frame — skip work when camera hasn't moved.
    pub last_cam_chunk: Option<ChunkCoord>,
    /// View radius in chunks.  13 chunks × 32 tiles = 416 tiles, which exceeds
    /// the camera's max zoom distance of 400 world units.
    pub render_radius: i32,
    /// Which layer (surface or underground tier) is currently being rendered.
    /// Changing this clears `lod_cache` so all chunks rebuild with new content.
    pub active_layer: ActiveLayer,
}

impl Default for ChunkManager {
    fn default() -> Self {
        Self {
            spawned:        HashMap::new(),
            lod_cache:      HashMap::new(),
            mesh_cache:     HashMap::new(),
            dirty:          HashSet::new(),
            last_cam_chunk: None,
            render_radius:  13,
            active_layer:   ActiveLayer::default(),
        }
    }
}

// ── TerrainAssets resource ────────────────────────────────────────────────────

/// Shared GPU handles inserted at startup.
#[derive(Resource)]
pub struct TerrainAssets {
    pub material: Handle<StandardMaterial>,
}

// ── System 0: active layer detection ─────────────────────────────────────────

/// Detect when the player's depth crosses a tier boundary and invalidate the
/// chunk cache so `rebuild_dirty_chunks` rebuilds all visible chunks with the
/// correct layer content (surface terrain or underground floor quads).
///
/// Runs before `update_chunk_visibility` so the layer is stable for the rest
/// of the frame's terrain pipeline.
pub fn sync_active_layer(
    camera_q: Query<&OrbitCamera>,
    mut mgr: ResMut<ChunkManager>,
) {
    let Ok(cam) = camera_q.single() else { return };
    let new_layer = ActiveLayer::from_camera_y(cam.target.y);
    if new_layer != mgr.active_layer {
        mgr.active_layer    = new_layer;
        mgr.lod_cache.clear();
        mgr.last_cam_chunk  = None;
        // mesh_cache is intentionally kept: cached meshes for the new layer
        // may already exist from a prior visit, avoiding redundant rebuilds.
    }
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

    let active = mgr.active_layer;

    for coord in dirty {
        let Some(&lod) = mgr.lod_cache.get(&coord) else { continue };

        let mesh = match active {
            ActiveLayer::Surface => {
                // Compute edge-transition flags from neighbour LODs.
                let transitions = EdgeTransitions {
                    north: is_coarser_neighbor(&mgr.lod_cache, coord,  0, -1, lod),
                    south: is_coarser_neighbor(&mgr.lod_cache, coord,  0,  1, lod),
                    west:  is_coarser_neighbor(&mgr.lod_cache, coord, -1,  0, lod),
                    east:  is_coarser_neighbor(&mgr.lod_cache, coord,  1,  0, lod),
                };
                build_chunk_mesh(&map, coord, lod, transitions)
            }
            ActiveLayer::Cave(kind) => build_underground_chunk_mesh(&map, coord, kind),
        };

        // Underground meshes are always cached under LodLevel::Full — flat floors
        // have no height seams so LOD stitching is unnecessary.
        let cache_lod = match active {
            ActiveLayer::Surface => lod,
            ActiveLayer::Cave(_) => LodLevel::Full,
        };
        let handle = meshes.add(mesh);
        mgr.mesh_cache.insert((coord, cache_lod, active), handle);
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
    mut commands: Commands,
    mut mgr:      ResMut<ChunkManager>,
    assets:       Res<TerrainAssets>,
) {
    // ── Despawn chunks no longer in lod_cache ─────────────────────────────────

    let visible: HashSet<ChunkCoord> = mgr.lod_cache.keys().copied().collect();
    let to_despawn: Vec<ChunkCoord> = mgr.spawned.keys()
        .filter(|k| !visible.contains(k))
        .copied()
        .collect();

    for coord in to_despawn {
        if let Some(entity) = mgr.spawned.remove(&coord) {
            commands.entity(entity).despawn();
        }
    }

    // ── Spawn or update visible chunks ────────────────────────────────────────

    let active = mgr.active_layer;
    let to_update: Vec<(ChunkCoord, LodLevel)> = mgr.lod_cache
        .iter()
        .map(|(&c, &l)| (c, l))
        .collect();

    for (coord, lod) in to_update {
        let cache_lod = match active {
            ActiveLayer::Surface => lod,
            ActiveLayer::Cave(_) => LodLevel::Full,
        };
        let Some(handle) = mgr.mesh_cache.get(&(coord, cache_lod, active)).cloned() else { continue };

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
        }
    }
}
