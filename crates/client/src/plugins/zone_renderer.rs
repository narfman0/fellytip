//! Zone interior mesh renderer.
//!
//! When the local player enters a zone (as reflected by `ZoneMembership`),
//! this plugin reads the cached `ZoneTileMessage` from `ZoneCache` and spawns
//! one PBR quad per tile — floor, wall billboard, balcony, roof — tinted by
//! the zone kind (above-ground warm brown, dungeon grey, underground near-black
//! with a bioluminescent tint).
//!
//! Current zone tiles are rendered on `RenderLayers::layer(0)`.
//! 1-hop neighbor zone tiles are rendered on `RenderLayers::layer(1)`.
//! 2-hop neighbor zone tiles are rendered on `RenderLayers::layer(2)`.
//!
//! Meshes from zones more than 2 hops from the player's current zone are
//! despawned. Overworld zone is rendered by the terrain chunk system; this
//! plugin skips it.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use fellytip_shared::world::zone::{InteriorTile, ZoneId, ZoneKind, ZoneMembership, OVERWORLD_ZONE};

use super::zone_cache::{ZoneCache, ZoneNeighborCache};
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
    /// Hop distance from the player's current zone at the time this mesh was
    /// spawned. 0 = current zone, 1 = 1-hop neighbor, 2 = 2-hop neighbor.
    #[allow(dead_code)]
    pub hop_distance: u8,
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

/// Spawn meshes for a zone at the given hop distance.
fn spawn_zone(
    zone_id: ZoneId,
    hop_distance: u8,
    cache: &ZoneCache,
    existing: &Query<&ZoneMeshMarker>,
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
) {
    if zone_id == OVERWORLD_ZONE {
        return;
    }

    // Already rendered?
    if existing.iter().any(|m| m.zone_id == zone_id) {
        return;
    }

    let Some(msg) = cache.0.get(&zone_id) else {
        return;
    };

    let render_layer = RenderLayers::layer(hop_distance as usize);

    let kind = msg.zone_kind;
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
                        ZoneMeshMarker { zone_id, hop_distance },
                        render_layer.clone(),
                    ));
                }
                InteriorTile::Wall | InteriorTile::Window => {
                    commands.spawn((
                        Mesh3d(wall_mesh.clone()),
                        MeshMaterial3d(wall_mat.clone()),
                        Transform::from_xyz(wx, 0.0, wz),
                        ZoneMeshMarker { zone_id, hop_distance },
                        render_layer.clone(),
                    ));
                }
                InteriorTile::Balcony => {
                    commands.spawn((
                        Mesh3d(floor_mesh.clone()),
                        MeshMaterial3d(balcony_mat.clone()),
                        // Slight elevation so it reads as a hanging ledge above a drop.
                        Transform::from_xyz(wx, 0.02, wz),
                        ZoneMeshMarker { zone_id, hop_distance },
                        render_layer.clone(),
                    ));
                }
                InteriorTile::Roof => {
                    commands.spawn((
                        Mesh3d(floor_mesh.clone()),
                        MeshMaterial3d(roof_mat.clone()),
                        Transform::from_xyz(wx, WALL_HEIGHT, wz),
                        ZoneMeshMarker { zone_id, hop_distance },
                        render_layer.clone(),
                    ));
                }
                InteriorTile::Void | InteriorTile::Pit => {
                    // Rendered as empty space — no mesh.
                }
            }
        }
    }

    tracing::info!(
        zone = ?zone_id,
        kind = ?kind,
        hop = hop_distance,
        tiles = msg.tiles.len(),
        "Spawned zone interior meshes"
    );
}

/// Spawn meshes for the player's current zone and neighbor zones (up to 2 hops)
/// when the zone changes or its cache entry arrives.
#[allow(clippy::too_many_arguments)]
fn spawn_zone_meshes(
    mut commands: Commands,
    cache: Res<ZoneCache>,
    neighbor_cache: Res<ZoneNeighborCache>,
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

    // Always try to render the current zone at hop 0.
    spawn_zone(
        player_zone,
        0,
        &cache,
        &existing,
        &mut commands,
        &mut meshes,
        &mut materials,
    );

    // Render neighbor zones based on topology from the neighbor cache.
    let Some(ref neighbor_msg) = neighbor_cache.0 else {
        return;
    };

    // Only act if the topology is for our current zone.
    if neighbor_msg.current_zone != player_zone {
        return;
    }

    for &(zone_id, hop) in &neighbor_msg.zone_hops {
        if hop == 0 {
            // Already handled above.
            continue;
        }
        if hop > 2 {
            continue;
        }
        spawn_zone(
            zone_id,
            hop,
            &cache,
            &existing,
            &mut commands,
            &mut meshes,
            &mut materials,
        );
    }
}

/// Despawn any `ZoneMeshMarker` entity whose zone is more than 2 hops from
/// the player's current zone, based on the neighbor cache.
fn despawn_stale_zone_meshes(
    mut commands: Commands,
    player_zone_q: Query<Option<&ZoneMembership>, With<LocalPlayer>>,
    neighbor_cache: Res<ZoneNeighborCache>,
    meshes: Query<(Entity, &ZoneMeshMarker)>,
) {
    let player_zone = player_zone_q
        .single()
        .ok()
        .and_then(|opt| opt.copied())
        .map(|z| z.0)
        .unwrap_or(OVERWORLD_ZONE);

    // Build the set of valid zone IDs (within 2 hops).
    let valid_zones: std::collections::HashSet<ZoneId> =
        if let Some(ref neighbor_msg) = neighbor_cache.0 {
            if neighbor_msg.current_zone == player_zone {
                neighbor_msg
                    .zone_hops
                    .iter()
                    .map(|(z, _)| *z)
                    .collect()
            } else {
                // Topology not yet updated for this zone — only keep current zone.
                std::iter::once(player_zone).collect()
            }
        } else {
            std::iter::once(player_zone).collect()
        };

    for (entity, marker) in &meshes {
        if !valid_zones.contains(&marker.zone_id) {
            commands.entity(entity).despawn();
        }
    }
}
