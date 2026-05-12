//! Per-chunk terrain trimesh colliders, driven by player position.
//!
//! Authoritative collider geometry lives here (game crate, runs in both
//! headless and windowed). The client renderer's chunk meshes are now purely
//! visual — physics is independent and works identically in `--headless`.
//!
//! Each active player has a square "physics disk" of `PHYS_CHUNK_RADIUS`
//! chunks around their `WorldPosition`. The union of all players' disks is
//! the set of active chunks; chunks outside everyone's disk get despawned.
//!
//! Each active chunk owns one `RigidBody::Static` + `Collider::trimesh`
//! entity built from `build_chunk_geometry` at LOD 0 (full resolution).

use std::collections::{HashMap, HashSet};

use avian3d::prelude::{Collider, RigidBody};
use bevy::prelude::*;
use fellytip_shared::components::WorldPosition;
use fellytip_shared::world::map::{WorldMap, CHUNK_TILES};
use fellytip_shared::world::mesh::{build_chunk_geometry, ChunkCoord, EdgeTransitions};

use super::bot::BotController;
use super::combat::LastPlayerInput;

/// Marker for entities the physics layer should track. Currently any entity
/// with `WorldPosition` is considered a player from physics' POV — bots and
/// the local player both qualify.
///
/// Radius (in chunks) around each tracked entity that should have physics
/// colliders loaded.
pub const PHYS_CHUNK_RADIUS: i32 = 6;

/// Map of chunk coord → entity holding the trimesh collider for that chunk.
#[derive(Resource, Default)]
pub struct PhysicsChunks {
    pub spawned: HashMap<ChunkCoord, Entity>,
}

pub struct PhysicsWorldPlugin;

impl Plugin for PhysicsWorldPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<PhysicsChunk>()
            .init_resource::<PhysicsChunks>()
            .add_systems(Update, update_physics_chunks);
    }
}

/// Active set computed from player positions; not stored, recomputed each frame.
fn active_chunks_for_players<'a>(
    players: impl IntoIterator<Item = &'a WorldPosition>,
    map: &WorldMap,
) -> HashSet<ChunkCoord> {
    let half_w = (map.width  / 2) as f32;
    let half_h = (map.height / 2) as f32;
    let n_chunks_x = map.width.div_ceil(CHUNK_TILES) as i32;
    let n_chunks_y = map.height.div_ceil(CHUNK_TILES) as i32;

    let mut set = HashSet::new();
    for pos in players {
        // WorldPosition uses (x, y) horizontal + z up. Convert to tile-grid.
        let tile_x = (pos.x + half_w) as i32;
        let tile_y = (pos.y + half_h) as i32;
        let center = ChunkCoord::from_tile(tile_x, tile_y);
        for dy in -PHYS_CHUNK_RADIUS..=PHYS_CHUNK_RADIUS {
            for dx in -PHYS_CHUNK_RADIUS..=PHYS_CHUNK_RADIUS {
                let c = ChunkCoord { cx: center.cx + dx, cy: center.cy + dy };
                if c.cx < 0 || c.cy < 0 || c.cx >= n_chunks_x || c.cy >= n_chunks_y {
                    continue;
                }
                set.insert(c);
            }
        }
    }
    set
}

/// Spawn/despawn trimesh colliders so the active set matches the union of all
/// player disks. Runs every Update — cheap because spawned set is keyed by
/// `ChunkCoord` and we only touch the symmetric difference.
#[allow(clippy::type_complexity)]
fn update_physics_chunks(
    mut commands: Commands,
    map: Option<Res<WorldMap>>,
    // Drive activation off the local player and any bots. NPCs are excluded —
    // they don't need physics colliders around them, and including them would
    // unify all their disks across the whole map.
    players: Query<&WorldPosition, Or<(With<LastPlayerInput>, With<BotController>)>>,
    mut chunks: ResMut<PhysicsChunks>,
) {
    let Some(map) = map else { return };
    if players.is_empty() { return }

    let active = active_chunks_for_players(players.iter(), &map);

    // Despawn no-longer-active chunks.
    let to_remove: Vec<ChunkCoord> = chunks.spawned.keys()
        .filter(|c| !active.contains(c))
        .copied()
        .collect();
    for c in to_remove {
        if let Some(entity) = chunks.spawned.remove(&c) {
            commands.entity(entity).despawn();
        }
    }

    // Spawn newly-active chunks. EdgeTransitions::default() = no LOD seams
    // since physics is always at LOD 0 / full resolution.
    for c in &active {
        if chunks.spawned.contains_key(c) { continue }
        let geom = build_chunk_geometry(&map, *c, 1, EdgeTransitions::default());
        // Convert positions Vec<[f32;3]> → Vec<Vec3> for avian's trimesh ctor.
        let verts: Vec<Vec3> = geom.positions.iter().map(|p| Vec3::from_array(*p)).collect();
        // Indices come as a flat Vec<u32>; avian wants Vec<[u32;3]>.
        let mut tris: Vec<[u32; 3]> = Vec::with_capacity(geom.indices.len() / 3);
        for tri in geom.indices.chunks_exact(3) {
            tris.push([tri[0], tri[1], tri[2]]);
        }
        let collider = Collider::trimesh(verts, tris);
        let entity = commands.spawn((
            RigidBody::Static,
            collider,
            Transform::IDENTITY,
            GlobalTransform::IDENTITY,
            PhysicsChunk::from(*c),
        )).id();
        chunks.spawned.insert(*c, entity);
    }
}

/// Tag component on physics chunk entities (handy for queries / debug tools).
///
/// `ChunkCoord` isn't reflectable, so we expose the components as plain
/// `i32` fields here for BRP reachability.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct PhysicsChunk {
    pub cx: i32,
    pub cy: i32,
}

impl From<ChunkCoord> for PhysicsChunk {
    fn from(c: ChunkCoord) -> Self { Self { cx: c.cx, cy: c.cy } }
}
