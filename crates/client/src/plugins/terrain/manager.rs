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

use avian3d::prelude::{Collider, ColliderConstructor, RigidBody};
use super::material::{tilekind_to_biome_region, BiomeRegion};

use bevy::prelude::*;
use fellytip_shared::world::map::WorldMap;

use super::chunk::{build_chunk_mesh, ChunkCoord};
use super::lod::{EdgeTransitions, LodLevel, CHUNK_TILES};
use fellytip_shared::world::map::TileKind;
use super::water_material::{build_water_mesh, WaterMaterialHandle, WaterOverlay};
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
    /// Water overlay entities (spawned alongside terrain chunks).
    pub water_spawned: HashMap<ChunkCoord, Entity>,
    /// Most-recent LOD assigned to each visible chunk.
    pub lod_cache: HashMap<ChunkCoord, LodLevel>,
    /// Cached mesh handles keyed by (coord, lod) to avoid rebuilding unchanged meshes.
    pub mesh_cache: HashMap<(ChunkCoord, LodLevel), Handle<Mesh>>,
    /// Cached water overlay mesh handles (built once per coord, LOD-independent).
    pub water_mesh_cache: HashMap<ChunkCoord, Handle<Mesh>>,
    /// Last mesh handle applied to each spawned chunk entity — used to skip
    /// redundant ECS updates (and suppress avian double-collider warnings).
    pub applied_mesh: HashMap<ChunkCoord, Handle<Mesh>>,
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
            spawned:          HashMap::new(),
            water_spawned:    HashMap::new(),
            lod_cache:        HashMap::new(),
            mesh_cache:       HashMap::new(),
            water_mesh_cache: HashMap::new(),
            applied_mesh:     HashMap::new(),
            dirty:            HashSet::new(),
            last_cam_chunk:   None,
            render_radius:    20,
        }
    }
}

// ── TerrainAssets resource ────────────────────────────────────────────────────

/// Shared GPU handles inserted at startup.
#[derive(Resource)]
pub struct TerrainAssets {
    /// One material per `BiomeRegion`, selected per-chunk based on dominant tile kind.
    pub biome_materials: HashMap<BiomeRegion, Handle<StandardMaterial>>,
}

/// Marker on surface terrain chunk entities.
#[derive(Component)]
pub struct SurfaceTerrain;

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

        // Build water overlay mesh (LOD-independent: always full resolution).
        if let std::collections::hash_map::Entry::Vacant(e) = mgr.water_mesh_cache.entry(coord) {
            if let Some(water_mesh) = build_water_mesh(&map, coord.cx, coord.cy) {
                e.insert(meshes.add(water_mesh));
            }
        }
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

/// Sample the dominant `TileKind` at a chunk's centre tile for material selection.
fn chunk_dominant_kind(map: &WorldMap, coord: ChunkCoord) -> TileKind {
    let ix = (coord.cx as usize) * CHUNK_TILES + CHUNK_TILES / 2;
    let iy = (coord.cy as usize) * CHUNK_TILES + CHUNK_TILES / 2;
    let ix = ix.min(map.width.saturating_sub(1));
    let iy = iy.min(map.height.saturating_sub(1));
    map.column(ix, iy)
        .layers
        .iter()
        .rev()
        .find(|l| l.kind != TileKind::Void)
        .map(|l| l.kind)
        .unwrap_or(TileKind::Grassland)
}

// ── System 3: ECS sync ────────────────────────────────────────────────────────

pub fn apply_chunk_meshes(
    mut commands:  Commands,
    mut mgr:       ResMut<ChunkManager>,
    assets:        Res<TerrainAssets>,
    map:           Res<WorldMap>,
    meshes:        Res<Assets<Mesh>>,
    water_mat:     Option<Res<WaterMaterialHandle>>,
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
        mgr.applied_mesh.remove(&coord);
        // Also despawn water overlay if it exists.
        if let Some(water_entity) = mgr.water_spawned.remove(&coord) {
            commands.entity(water_entity).despawn();
        }
    }

    // ── Spawn or update visible chunks ────────────────────────────────────────

    let to_update: Vec<(ChunkCoord, LodLevel)> = mgr.lod_cache
        .iter()
        .map(|(&c, &l)| (c, l))
        .collect();

    for (coord, lod) in to_update {
        let Some(handle) = mgr.mesh_cache.get(&(coord, lod)).cloned() else { continue };

        // Skip if the same mesh is already applied — avian would warn about
        // a duplicate ColliderConstructor on an entity that already has a Collider.
        if mgr.applied_mesh.get(&coord) == Some(&handle) {
            continue;
        }

        // Pick material from the dominant biome at the chunk centre.
        let region = tilekind_to_biome_region(chunk_dominant_kind(&map, coord));
        let mat = assets
            .biome_materials
            .get(&region)
            .or_else(|| assets.biome_materials.values().next())
            .expect("TerrainAssets must have at least one biome material")
            .clone();

        if let Some(&entity) = mgr.spawned.get(&coord) {
            // LOD changed — build the new collider synchronously so the old one
            // stays active until it is atomically replaced.  rebuild_dirty_chunks
            // runs before this system, so the mesh is already in Assets<Mesh>.
            let mut cmd = commands.entity(entity);
            cmd.insert((Mesh3d(handle.clone()), MeshMaterial3d(mat)));
            if let Some(collider) = meshes.get(&handle).and_then(Collider::trimesh_from_mesh) {
                cmd.insert(collider);
            } else {
                // Mesh not in Assets yet (shouldn't happen) — fall back to deferred
                // constructor, accepting the 1-frame gap rather than leaving no collider.
                cmd.remove::<Collider>().insert(ColliderConstructor::TrimeshFromMesh);
            }
        } else {
            // Spawn a new chunk entity. Vertex positions are in world space,
            // so Transform::IDENTITY places it correctly with no offset.
            let entity = commands.spawn((
                Mesh3d(handle.clone()),
                MeshMaterial3d(mat),
                Transform::IDENTITY,
                SurfaceTerrain,
                RigidBody::Static,
                ColliderConstructor::TrimeshFromMesh,
            )).id();
            mgr.spawned.insert(coord, entity);
            lifecycle.newly_visible.push((coord, entity));

            // Spawn water overlay if this chunk has water tiles and we have the material.
            if let Some(ref wmat) = water_mat {
                if let Some(water_handle) = mgr.water_mesh_cache.get(&coord).cloned() {
                    let water_entity = commands.spawn((
                        Mesh3d(water_handle),
                        MeshMaterial3d(wmat.0.clone()),
                        Transform::IDENTITY,
                        WaterOverlay,
                    )).id();
                    mgr.water_spawned.insert(coord, water_entity);
                }
            }
        }
        mgr.applied_mesh.insert(coord, handle);
    }
}

/// Show/hide surface terrain chunks based on whether the local player is on the surface.
///
/// Underground (in WORLD_SUNKEN_REALM): hide surface chunks.
/// Surface: show surface chunks.
pub fn update_layer_visibility(
    player_q: Query<(&PredictedPosition, Option<&fellytip_shared::world::zone::ZoneMembership>), With<LocalPlayer>>,
    zone_registry: Option<Res<fellytip_shared::world::zone::ZoneRegistry>>,
    mut surface_q: Query<&mut Visibility, With<SurfaceTerrain>>,
) {
    let Ok((pos, zone_membership)) = player_q.single() else { return };

    // Check world_id via ZoneMembership → ZoneRegistry first; fall back to z-check.
    let is_underground = if let (Some(registry), Some(membership)) = (&zone_registry, zone_membership) {
        registry.get(membership.0)
            .map(|z| z.world_id == fellytip_shared::world::zone::WORLD_SUNKEN_REALM)
            .unwrap_or(false)
    } else {
        pos.z < -1.0
    };

    let surface_vis = if is_underground { Visibility::Hidden } else { Visibility::Visible };
    for mut v in &mut surface_q {
        if *v != surface_vis { *v = surface_vis; }
    }
}
