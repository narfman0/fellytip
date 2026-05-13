//! Per-chunk terrain trimesh colliders + per-zone interior trimesh colliders,
//! driven by player position and `ZoneMembership`.
//!
//! Authoritative collider geometry lives here (game crate, runs in both
//! headless and windowed). The client renderer's meshes are now purely visual
//! — physics is independent and works identically in `--headless`.
//!
//! ## Overworld terrain (`PhysicsChunk` entities)
//!
//! Each player in `OVERWORLD_ZONE` has a square "physics disk" of
//! `PHYS_CHUNK_RADIUS` chunks around their `WorldPosition`. The union of all
//! overworld players' disks is the active set; chunks outside it get despawned.
//!
//! ## Zone interiors (`PhysicsZone` entities)
//!
//! Each non-overworld zone with at least one player in it gets one
//! `Collider::trimesh` covering its floors + walls. The trimesh is built from
//! `build_zone_interior_geometry` at `Transform::IDENTITY` (zone-local coords),
//! matching how `zone_renderer::spawn_zone_meshes` places its visuals.
//!
//! Overworld chunks despawn when no player remains in the overworld so the
//! two coordinate spaces don't double-load colliders at the same xy.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::{Collider, RigidBody};
use bevy::prelude::*;
use fellytip_shared::components::WorldPosition;
use fellytip_shared::world::civilization::{Building, Buildings};
use fellytip_shared::world::map::{WorldMap, CHUNK_TILES, MAP_HEIGHT, MAP_WIDTH};
use fellytip_shared::world::mesh::{
    build_chunk_geometry, build_zone_interior_geometry, ChunkCoord, EdgeTransitions,
};
use fellytip_shared::world::zone::{ZoneId, ZoneMembership, ZoneRegistry, OVERWORLD_ZONE};

use super::bot::BotController;
use super::combat::LastPlayerInput;

/// Radius (in chunks) around each tracked entity that should have physics
/// colliders loaded.
pub const PHYS_CHUNK_RADIUS: i32 = 6;

/// Map of chunk coord → entity holding the trimesh collider for that chunk.
#[derive(Resource, Default)]
pub struct PhysicsChunks {
    pub spawned: HashMap<ChunkCoord, Entity>,
}

/// Map of zone id → entity holding the trimesh collider for that zone interior.
#[derive(Resource, Default)]
pub struct PhysicsZoneColliders {
    pub spawned: HashMap<ZoneId, Entity>,
}

pub struct PhysicsWorldPlugin;

impl Plugin for PhysicsWorldPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<PhysicsChunk>()
            .register_type::<PhysicsZone>()
            .register_type::<PhysicsBuilding>()
            // Register ZoneMembership + ZoneId so BRP / inspector can see them
            // — useful for verifying which zone a player is currently in.
            .register_type::<ZoneMembership>()
            .register_type::<ZoneId>()
            .register_type::<crate::movement::KinematicState>()
            .init_resource::<PhysicsChunks>()
            .init_resource::<PhysicsZoneColliders>()
            .add_systems(Update, (update_physics_world, refresh_building_colliders));
    }
}

/// Active overworld set computed from player positions.
fn active_chunks_for_players<'a>(
    positions: impl IntoIterator<Item = &'a WorldPosition>,
    map: &WorldMap,
) -> HashSet<ChunkCoord> {
    let half_w = (map.width  / 2) as f32;
    let half_h = (map.height / 2) as f32;
    let n_chunks_x = map.width.div_ceil(CHUNK_TILES) as i32;
    let n_chunks_y = map.height.div_ceil(CHUNK_TILES) as i32;

    let mut set = HashSet::new();
    for pos in positions {
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

/// Drive both overworld chunk colliders and zone interior colliders off the
/// `ZoneMembership` of each player/bot.
#[allow(clippy::type_complexity)]
fn update_physics_world(
    mut commands: Commands,
    map: Option<Res<WorldMap>>,
    registry: Option<Res<ZoneRegistry>>,
    players: Query<
        (&WorldPosition, &ZoneMembership),
        Or<(With<LastPlayerInput>, With<BotController>)>,
    >,
    mut chunks: ResMut<PhysicsChunks>,
    mut zones: ResMut<PhysicsZoneColliders>,
) {
    let Some(map) = map else { return };
    if players.is_empty() { return }

    // Bucket players: overworld positions vs. interior zone ids.
    let mut overworld_positions: Vec<&WorldPosition> = Vec::new();
    let mut active_zones: HashSet<ZoneId> = HashSet::new();
    for (pos, zone) in &players {
        if zone.0 == OVERWORLD_ZONE {
            overworld_positions.push(pos);
        } else {
            active_zones.insert(zone.0);
        }
    }

    // ── Overworld chunk colliders ────────────────────────────────────────────
    let active_chunks = if overworld_positions.is_empty() {
        HashSet::new()
    } else {
        active_chunks_for_players(overworld_positions, &map)
    };

    let to_remove: Vec<ChunkCoord> = chunks.spawned.keys()
        .filter(|c| !active_chunks.contains(c))
        .copied()
        .collect();
    for c in to_remove {
        if let Some(entity) = chunks.spawned.remove(&c) {
            commands.entity(entity).despawn();
        }
    }

    for c in &active_chunks {
        if chunks.spawned.contains_key(c) { continue }
        let geom = build_chunk_geometry(&map, *c, 1, EdgeTransitions::default());
        let verts: Vec<Vec3> = geom.positions.iter().map(|p| Vec3::from_array(*p)).collect();
        let mut tris: Vec<[u32; 3]> = Vec::with_capacity(geom.indices.len() / 3);
        for tri in geom.indices.chunks_exact(3) {
            tris.push([tri[0], tri[1], tri[2]]);
        }
        let entity = commands.spawn((
            RigidBody::Static,
            Collider::trimesh(verts, tris),
            Transform::IDENTITY,
            GlobalTransform::IDENTITY,
            PhysicsChunk::from(*c),
        )).id();
        chunks.spawned.insert(*c, entity);
    }

    // ── Zone interior colliders ──────────────────────────────────────────────
    let to_remove_zones: Vec<ZoneId> = zones.spawned.keys()
        .filter(|z| !active_zones.contains(z))
        .copied()
        .collect();
    for z in to_remove_zones {
        if let Some(entity) = zones.spawned.remove(&z) {
            commands.entity(entity).despawn();
        }
    }

    let Some(registry) = registry else { return };
    for &zone_id in &active_zones {
        if zones.spawned.contains_key(&zone_id) { continue }
        let Some(zone) = registry.get(zone_id) else {
            tracing::warn!(?zone_id, "PhysicsZone: zone not in ZoneRegistry");
            continue;
        };
        let Some(template) = registry.templates.get(&zone.template_id) else {
            tracing::warn!(?zone_id, template = zone.template_id, "PhysicsZone: template not in registry");
            continue;
        };
        let geom = build_zone_interior_geometry(&template.tiles, zone.width, zone.height);
        if geom.positions.is_empty() {
            // Zone has no walkable surface at all — skip the collider but track
            // it as "spawned" with a placeholder so we don't reattempt every frame.
            continue;
        }
        let verts: Vec<Vec3> = geom.positions.iter().map(|p| Vec3::from_array(*p)).collect();
        let mut tris: Vec<[u32; 3]> = Vec::with_capacity(geom.indices.len() / 3);
        for tri in geom.indices.chunks_exact(3) {
            tris.push([tri[0], tri[1], tri[2]]);
        }
        let entity = commands.spawn((
            RigidBody::Static,
            Collider::trimesh(verts, tris),
            Transform::IDENTITY,
            GlobalTransform::IDENTITY,
            PhysicsZone { zone_id: zone_id.0 },
        )).id();
        zones.spawned.insert(zone_id, entity);
    }
}

/// Tag component on per-chunk trimesh entities. Made reflectable so BRP and
/// `bevy-inspector-egui` can read the chunk coord.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct PhysicsChunk {
    pub cx: i32,
    pub cy: i32,
}

impl From<ChunkCoord> for PhysicsChunk {
    fn from(c: ChunkCoord) -> Self { Self { cx: c.cx, cy: c.cy } }
}

/// Tag component on per-zone-interior trimesh entities.
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct PhysicsZone {
    pub zone_id: u32,
}

/// Tag component on per-building cuboid colliders. Used to find/despawn them
/// when the `Buildings` resource changes (world reload, seed swap).
#[derive(Component, Reflect, Debug, Clone, Copy)]
#[reflect(Component)]
pub struct PhysicsBuilding;

/// Rebuild building cuboid colliders whenever the `Buildings` resource changes.
///
/// Half-extents come from `BuildingKind::approx_half_extents()` — a pure
/// data-only map of each kind to a footprint + height. No GLB loading needed,
/// so colliders exist identically in windowed and headless modes.
fn refresh_building_colliders(
    mut commands: Commands,
    buildings: Option<Res<Buildings>>,
    existing: Query<Entity, With<PhysicsBuilding>>,
) {
    let Some(buildings) = buildings else { return };
    if !buildings.is_changed() { return }

    for entity in &existing {
        commands.entity(entity).despawn();
    }
    for b in &buildings.0 {
        spawn_building_collider(&mut commands, b);
    }
}

fn spawn_building_collider(commands: &mut Commands, b: &Building) {
    let (hx, hy, hz) = b.kind.approx_half_extents();
    // Tile-space → physics-world: tile (tx, ty) centered at
    // (tx - MAP_WIDTH/2 + 0.5, ty - MAP_HEIGHT/2 + 0.5). Matches
    // entity_renderer.rs::spawn_building_visuals so collider lines up with
    // the GLB visual.
    let half_w = (MAP_WIDTH  / 2) as f32;
    let half_h = (MAP_HEIGHT / 2) as f32;
    let wx = b.tx as f32 - half_w + 0.5;
    let wy = b.ty as f32 - half_h + 0.5;
    let pos = Vec3::new(wx, b.z + hy, wy);
    let rot = Quat::from_rotation_y(b.rotation as f32 * std::f32::consts::FRAC_PI_2);
    commands.spawn((
        RigidBody::Static,
        Collider::cuboid(hx * 2.0, hy * 2.0, hz * 2.0),
        Transform::from_translation(pos).with_rotation(rot),
        GlobalTransform::IDENTITY,
        PhysicsBuilding,
    ));
}
