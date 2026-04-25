//! Zone interior mesh renderer.
//!
//! When the local player enters a zone (as reflected by `ZoneMembership`),
//! this plugin reads the cached `ZoneTileMessage` from `ZoneCache` and spawns
//! one PBR quad per tile — floor, wall billboard, balcony, roof — tinted by
//! the zone kind (above-ground warm brown, dungeon grey, underground near-black
//! with a bioluminescent tint).
//!
//! Meshes from other zones are despawned, so the interior rendering stays
//! local to the player's current zone. Overworld zone is rendered by the
//! terrain chunk system; this plugin skips it.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use fellytip_shared::world::zone::{
    InteriorTile, ZoneId, ZoneKind, ZoneMembership, OVERWORLD_ZONE,
};

use super::zone_cache::ZoneCache;
use crate::LocalPlayer;

/// Side length of a single interior tile in world units.
const TILE_SIZE: f32 = 1.0;
/// Vertical thickness for wall billboards.
const WALL_HEIGHT: f32 = 2.5;

/// Component tagging spawned zone-mesh entities so they can be despawned in
/// bulk when the player moves to a different zone.
#[derive(Component)]
pub struct ZoneMeshMarker {
    pub zone_id: ZoneId,
}

pub struct ZoneRendererPlugin;

impl Plugin for ZoneRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (spawn_zone_meshes, despawn_stale_zone_meshes));
    }
}

/// Produce a single-quad horizontal floor mesh of `TILE_SIZE × TILE_SIZE`
/// centered on the origin on the XZ plane.
fn floor_quad_mesh() -> Mesh {
    let half = TILE_SIZE * 0.5;
    let positions = vec![
        [-half, 0.0, -half],
        [ half, 0.0, -half],
        [ half, 0.0,  half],
        [-half, 0.0,  half],
    ];
    let normals = vec![[0.0, 1.0, 0.0]; 4];
    let uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let indices = vec![0, 1, 2, 0, 2, 3];
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Vertical wall quad of `TILE_SIZE × WALL_HEIGHT`, facing +Z.
fn wall_quad_mesh() -> Mesh {
    let half_w = TILE_SIZE * 0.5;
    let positions = vec![
        [-half_w, 0.0,         0.0],
        [ half_w, 0.0,         0.0],
        [ half_w, WALL_HEIGHT, 0.0],
        [-half_w, WALL_HEIGHT, 0.0],
    ];
    let normals = vec![[0.0, 0.0, 1.0]; 4];
    let uvs = vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let indices = vec![0, 1, 2, 0, 2, 3];
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Color tints keyed by zone kind.
fn floor_color(kind: ZoneKind) -> Color {
    match kind {
        ZoneKind::Overworld => Color::srgb(0.45, 0.35, 0.2),
        ZoneKind::BuildingFloor { .. } => Color::srgb(0.55, 0.40, 0.25), // warm brown
        ZoneKind::Dungeon { .. } => Color::srgb(0.35, 0.35, 0.38),      // grey stone
        ZoneKind::Underground { .. } => Color::srgb(0.08, 0.08, 0.14),    // near-black
    }
}

fn wall_color(kind: ZoneKind) -> Color {
    match kind {
        ZoneKind::Overworld => Color::srgb(0.3, 0.25, 0.18),
        ZoneKind::BuildingFloor { .. } => Color::srgb(0.40, 0.28, 0.18),
        ZoneKind::Dungeon { .. } => Color::srgb(0.24, 0.24, 0.26),
        ZoneKind::Underground { .. } => Color::srgb(0.04, 0.04, 0.09),
    }
}

fn roof_color(kind: ZoneKind) -> Color {
    match kind {
        ZoneKind::Underground { .. } => Color::srgb(0.05, 0.05, 0.10),
        _ => Color::srgb(0.2, 0.15, 0.1),
    }
}

/// Emissive tint for bioluminescent underground (Sunken Realm) atmosphere. Zero for all others.
fn emissive_for(kind: ZoneKind) -> LinearRgba {
    match kind {
        ZoneKind::Underground { .. } => LinearRgba::new(0.05, 0.12, 0.18, 0.0),
        _ => LinearRgba::new(0.0, 0.0, 0.0, 0.0),
    }
}

/// Determine the zone kind for a given `ZoneId` from cached server data.
/// Until we cache the kind explicitly, classify from the `ZoneId` via the
/// known Underground / BuildingFloor ranges encoded in the tile layout (anchors
/// and tile shapes differ enough). For now we cache the kind *per message*
/// by inspecting tile distribution heuristically.
fn classify_zone(id: ZoneId, tiles: &[InteriorTile]) -> ZoneKind {
    if id == OVERWORLD_ZONE {
        return ZoneKind::Overworld;
    }
    // Heuristic: a pure-floor 16×16 (256) grid matches the underground template.
    if tiles.len() == 256 && tiles.iter().all(|t| matches!(t, InteriorTile::Floor)) {
        return ZoneKind::Underground { depth: 1 };
    }
    // Fallback to a generic building interior.
    ZoneKind::BuildingFloor { floor: 0 }
}

/// Spawn meshes for the player's current zone when the zone changes or its
/// cache entry arrives.
#[allow(clippy::too_many_arguments)]
fn spawn_zone_meshes(
    mut commands: Commands,
    cache: Res<ZoneCache>,
    player_zone_q: Query<Option<&ZoneMembership>, With<LocalPlayer>>,
    existing: Query<&ZoneMeshMarker>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let player_zone = player_zone_q
        .single()
        .ok()
        .and_then(|opt| opt.copied())
        .map(|z| z.0)
        .unwrap_or(OVERWORLD_ZONE);

    if player_zone == OVERWORLD_ZONE {
        return;
    }

    // Already rendered?
    if existing.iter().any(|m| m.zone_id == player_zone) {
        return;
    }

    let Some(msg) = cache.0.get(&player_zone) else {
        return;
    };

    let kind = classify_zone(msg.zone_id, &msg.tiles);
    let floor_mat = materials.add(StandardMaterial {
        base_color: floor_color(kind),
        emissive: emissive_for(kind),
        perceptual_roughness: 0.9,
        ..default()
    });
    let wall_mat = materials.add(StandardMaterial {
        base_color: wall_color(kind),
        emissive: emissive_for(kind),
        perceptual_roughness: 0.85,
        ..default()
    });
    let balcony_mat = materials.add(StandardMaterial {
        base_color: floor_color(kind).with_alpha(0.6),
        emissive: emissive_for(kind),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.7,
        ..default()
    });
    let roof_mat = materials.add(StandardMaterial {
        base_color: roof_color(kind),
        perceptual_roughness: 0.8,
        ..default()
    });

    let floor_mesh = meshes.add(floor_quad_mesh());
    let wall_mesh = meshes.add(wall_quad_mesh());

    let w = msg.width as usize;
    let h = msg.height as usize;

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let Some(tile) = msg.tiles.get(idx) else {
                continue;
            };
            let wx = x as f32 * TILE_SIZE;
            let wz = y as f32 * TILE_SIZE;

            match tile {
                InteriorTile::Floor | InteriorTile::Stair | InteriorTile::Water => {
                    commands.spawn((
                        Mesh3d(floor_mesh.clone()),
                        MeshMaterial3d(floor_mat.clone()),
                        Transform::from_xyz(wx, 0.0, wz),
                        ZoneMeshMarker { zone_id: player_zone },
                    ));
                }
                InteriorTile::Wall | InteriorTile::Window => {
                    commands.spawn((
                        Mesh3d(wall_mesh.clone()),
                        MeshMaterial3d(wall_mat.clone()),
                        Transform::from_xyz(wx, 0.0, wz),
                        ZoneMeshMarker { zone_id: player_zone },
                    ));
                }
                InteriorTile::Balcony => {
                    commands.spawn((
                        Mesh3d(floor_mesh.clone()),
                        MeshMaterial3d(balcony_mat.clone()),
                        // Slight elevation so it reads as a hanging ledge above a drop.
                        Transform::from_xyz(wx, 0.02, wz),
                        ZoneMeshMarker { zone_id: player_zone },
                    ));
                }
                InteriorTile::Roof => {
                    commands.spawn((
                        Mesh3d(floor_mesh.clone()),
                        MeshMaterial3d(roof_mat.clone()),
                        Transform::from_xyz(wx, WALL_HEIGHT, wz),
                        ZoneMeshMarker { zone_id: player_zone },
                    ));
                }
                InteriorTile::Void | InteriorTile::Pit => {
                    // Rendered as empty space — no mesh.
                }
            }
        }
    }

    tracing::info!(
        zone = ?player_zone,
        kind = ?kind,
        tiles = msg.tiles.len(),
        "Spawned zone interior meshes"
    );
}

/// Despawn any `ZoneMeshMarker` entity whose `zone_id` differs from the
/// player's current zone.
fn despawn_stale_zone_meshes(
    mut commands: Commands,
    player_zone_q: Query<Option<&ZoneMembership>, With<LocalPlayer>>,
    meshes: Query<(Entity, &ZoneMeshMarker)>,
) {
    let player_zone = player_zone_q
        .single()
        .ok()
        .and_then(|opt| opt.copied())
        .map(|z| z.0)
        .unwrap_or(OVERWORLD_ZONE);

    for (entity, marker) in &meshes {
        if marker.zone_id != player_zone {
            commands.entity(entity).despawn();
        }
    }
}
