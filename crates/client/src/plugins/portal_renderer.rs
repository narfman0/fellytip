//! Portal preview rendering — Phase 1.
//!
//! For each portal in the player's current zone (and portals in neighbor zones),
//! this plugin:
//!
//! 1. Spawns a transparent quad (the portal "window") at the portal's from-anchor
//!    position using a simple unlit transparent material.
//! 2. Spawns a thin colored frame around the portal shape using a border mesh.
//! 3. Spawns a secondary `Camera3d` pointed at the destination zone (`to_anchor`
//!    position), rendering onto a 256×256 `RenderTarget::Image`.
//! 4. Applies that image as the portal window material so the player sees a
//!    live preview of the destination.
//!
//! Default portal shape: rectangle, width = `trigger_radius`, height = `trigger_radius * 2`.
//! Custom shape: if `portal.shape` is `Some(verts)`, those vertices are used.
//!
//! A global cap of `MAX_PORTAL_CAMERAS = 8` limits secondary camera count.

use bevy::{
    asset::RenderAssetUsages,
    camera::{RenderTarget, visibility::RenderLayers},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::render_resource::{
        Extent3d, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    },
};

use fellytip_shared::world::zone::{ZoneMembership, OVERWORLD_ZONE};

use super::zone_cache::ZoneNeighborCache;
use crate::LocalPlayer;

/// Maximum number of portal preview cameras that can be active simultaneously.
const MAX_PORTAL_CAMERAS: usize = 8;

/// Resolution of each portal preview render target.
const PORTAL_RT_SIZE: u32 = 256;

/// Frame tint color: faint blue-white, barely visible.
const PORTAL_FRAME_COLOR: Color = Color::srgba(0.8, 0.8, 1.0, 0.3);

/// Debug highlight color: bright yellow-orange emissive.
const PORTAL_DEBUG_EMISSIVE: LinearRgba = LinearRgba::new(4.0, 2.0, 0.0, 1.0);

// ── Resources ─────────────────────────────────────────────────────────────────

/// When `true`, all portal meshes are rendered with a bright emissive highlight
/// so they are impossible to miss. Toggle via `dm/set_portal_debug` BRP method.
#[derive(Resource, Default)]
pub struct PortalDebugOverlay(pub bool);

// ── Marker components ─────────────────────────────────────────────────────────

/// Tags the transparent portal window mesh entity and its frame mesh.
#[derive(Component)]
pub struct PortalMeshMarker {
    pub portal_id: u32,
}

/// Tags the portal camera entity.
#[derive(Component)]
pub struct PortalCameraMarker {
    pub portal_id: u32,
}

// ── Plugin ────────────────────────────────────────────────────────────────────

pub struct PortalRendererPlugin;

impl Plugin for PortalRendererPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PortalDebugOverlay>()
            .add_systems(
                Update,
                (despawn_stale_portal_meshes, spawn_portal_meshes, update_portal_debug_overlay).chain(),
            );
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build the 4 corner vertices of a portal rectangle centered at the origin
/// on the XZ plane (width = `trigger_radius`, height = `trigger_radius * 2`).
fn portal_rect_verts(trigger_radius: f32) -> [Vec2; 4] {
    let hw = trigger_radius * 0.5;
    let hh = trigger_radius;
    [
        Vec2::new(-hw, -hh),
        Vec2::new( hw, -hh),
        Vec2::new( hw,  hh),
        Vec2::new(-hw,  hh),
    ]
}

/// Build a flat (Y=0) quad mesh for the portal window from 2-D vertices.
/// Vertices are assumed to define a convex polygon in the XZ plane.
fn portal_window_mesh(verts: &[Vec2]) -> Option<Mesh> {
    if verts.len() < 3 {
        return None;
    }
    // Fan triangulation (works for convex polygons).
    let positions: Vec<[f32; 3]> = verts.iter().map(|v| [v.x, 0.0, v.y]).collect();
    let normals = vec![[0.0, 1.0, 0.0]; positions.len()];
    let uvs: Vec<[f32; 2]> = (0..positions.len())
        .map(|i| {
            let t = i as f32 / positions.len() as f32;
            [t, 0.5]
        })
        .collect();
    let mut indices: Vec<u32> = Vec::new();
    for i in 1..(positions.len() as u32 - 1) {
        indices.extend_from_slice(&[0, i, i + 1]);
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

/// Build a thin frame (line loop) mesh from 2-D vertices in the XZ plane.
/// Each segment is rendered as a thin quad of width `thickness`.
fn portal_frame_mesh(verts: &[Vec2], thickness: f32) -> Option<Mesh> {
    if verts.len() < 2 {
        return None;
    }
    let n = verts.len();
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        let dir = (b - a).normalize_or_zero();
        let perp = Vec2::new(-dir.y, dir.x) * (thickness * 0.5);

        let base = positions.len() as u32;

        // 4 corners of the segment quad (Y=0 plane).
        positions.push([a.x - perp.x, 0.0, a.y - perp.y]);
        positions.push([a.x + perp.x, 0.0, a.y + perp.y]);
        positions.push([b.x + perp.x, 0.0, b.y + perp.y]);
        positions.push([b.x - perp.x, 0.0, b.y - perp.y]);

        for _ in 0..4 {
            normals.push([0.0, 1.0, 0.0]);
            uvs.push([0.0, 0.0]);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

/// Create a 256×256 RGBA render target image for a portal camera.
fn create_portal_render_target(images: &mut ResMut<Assets<Image>>) -> Handle<Image> {
    let size = Extent3d {
        width: PORTAL_RT_SIZE,
        height: PORTAL_RT_SIZE,
        depth_or_array_layers: 1,
    };
    let mut image = Image {
        texture_descriptor: TextureDescriptor {
            label: None,
            size,
            dimension: TextureDimension::D2,
            format: TextureFormat::Bgra8UnormSrgb,
            mip_level_count: 1,
            sample_count: 1,
            usage: TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_DST
                | TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        },
        ..default()
    };
    image.resize(size);
    images.add(image)
}

// ── Systems ───────────────────────────────────────────────────────────────────

/// Despawn portal mesh and camera entities that are no longer in the player's
/// current zone or no longer referenced by the neighbor cache.
fn despawn_stale_portal_meshes(
    mut commands: Commands,
    player_zone_q: Query<Option<&ZoneMembership>, With<LocalPlayer>>,
    neighbor_cache: Res<ZoneNeighborCache>,
    portal_meshes: Query<(Entity, &PortalMeshMarker)>,
    portal_cameras: Query<(Entity, &PortalCameraMarker)>,
) {
    let player_zone = player_zone_q
        .single()
        .ok()
        .and_then(|opt| opt.copied())
        .map(|z| z.0)
        .unwrap_or(OVERWORLD_ZONE);

    // Collect valid portal IDs (portals from zones within 2 hops).
    let valid_portal_ids: std::collections::HashSet<u32> =
        if let Some(ref msg) = neighbor_cache.0 {
            if msg.current_zone == player_zone {
                msg.portals.iter().map(|e| e.portal.id).collect()
            } else {
                std::collections::HashSet::new()
            }
        } else {
            std::collections::HashSet::new()
        };

    for (entity, marker) in &portal_meshes {
        if !valid_portal_ids.contains(&marker.portal_id) {
            commands.entity(entity).despawn();
        }
    }
    for (entity, marker) in &portal_cameras {
        if !valid_portal_ids.contains(&marker.portal_id) {
            commands.entity(entity).despawn();
        }
    }
}

/// Apply or remove the debug emissive highlight on all portal meshes based on
/// the `PortalDebugOverlay` resource. Runs every frame; cheap when the overlay
/// state hasn't changed because Bevy's change-detection skips the asset write.
fn update_portal_debug_overlay(
    overlay: Res<PortalDebugOverlay>,
    portal_meshes: Query<&MeshMaterial3d<StandardMaterial>, With<PortalMeshMarker>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for mat_handle in &portal_meshes {
        if let Some(mat) = materials.get_mut(mat_handle.id()) {
            if overlay.0 {
                mat.emissive = PORTAL_DEBUG_EMISSIVE;
                mat.alpha_mode = AlphaMode::Opaque;
            } else {
                mat.emissive = LinearRgba::BLACK;
                mat.alpha_mode = AlphaMode::Blend;
            }
        }
    }
}

/// Spawn portal window mesh, frame, and camera for each portal not yet rendered.
#[allow(clippy::too_many_arguments)]
fn spawn_portal_meshes(
    mut commands: Commands,
    player_zone_q: Query<Option<&ZoneMembership>, With<LocalPlayer>>,
    neighbor_cache: Res<ZoneNeighborCache>,
    existing_meshes: Query<&PortalMeshMarker>,
    existing_cameras: Query<&PortalCameraMarker>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
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

    let Some(ref neighbor_msg) = neighbor_cache.0 else {
        return;
    };

    if neighbor_msg.current_zone != player_zone {
        return;
    }

    // Count existing cameras to enforce the global cap.
    let mut camera_count = existing_cameras.iter().count();

    for entry in &neighbor_msg.portals {
        let portal = &entry.portal;

        // Skip portals already rendered.
        if existing_meshes
            .iter()
            .any(|m| m.portal_id == portal.id)
        {
            continue;
        }

        // Portal window vertices (XZ plane).
        let verts: Vec<Vec2> = match &portal.shape {
            Some(custom) => custom.clone(),
            None => portal_rect_verts(portal.trigger_radius).to_vec(),
        };

        // Position: from-anchor position (Vec3::ZERO for Phase 1 — anchor world-space
        // positions will be wired in Phase 2).
        let anchor_pos = Vec3::ZERO;

        // Spawn window mesh with render target material.
        if let Some(window_mesh) = portal_window_mesh(&verts) {
            // Create a render target image for this portal.
            let rt_image = create_portal_render_target(&mut images);

            // Unlit transparent material using the render target as texture.
            let window_mat = materials.add(StandardMaterial {
                base_color: Color::WHITE,
                base_color_texture: Some(rt_image.clone()),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                ..default()
            });

            commands.spawn((
                Mesh3d(meshes.add(window_mesh)),
                MeshMaterial3d(window_mat),
                Transform::from_translation(anchor_pos),
                RenderLayers::layer(0),
                PortalMeshMarker { portal_id: portal.id },
            ));

            // Spawn frame mesh on top of window.
            if let Some(frame_mesh) = portal_frame_mesh(&verts, 0.05) {
                let frame_mat = materials.add(StandardMaterial {
                    base_color: PORTAL_FRAME_COLOR,
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                });
                commands.spawn((
                    Mesh3d(meshes.add(frame_mesh)),
                    MeshMaterial3d(frame_mat),
                    Transform::from_translation(anchor_pos + Vec3::Y * 0.01),
                    RenderLayers::layer(0),
                    PortalMeshMarker { portal_id: portal.id },
                ));
            }

            // Spawn portal camera if under the cap.
            if camera_count < MAX_PORTAL_CAMERAS {
                // Camera render layer: destination zone hop distance.
                // from_hop = 0 means the portal is in the current zone, so the
                // destination is 1 hop away → layer(1).
                let dest_hop = entry.from_hop + 1;
                let camera_layer = dest_hop.min(2);

                // Portal camera positioned at to_anchor (Vec3::ZERO for Phase 1).
                let camera_pos = Vec3::ZERO;

                commands.spawn((
                    Camera3d::default(),
                    Camera {
                        is_active: true,
                        order: -1,
                        ..default()
                    },
                    RenderTarget::from(rt_image),
                    Transform::from_translation(camera_pos).looking_at(Vec3::ZERO, Vec3::Y),
                    RenderLayers::layer(camera_layer as usize),
                    PortalCameraMarker { portal_id: portal.id },
                ));

                camera_count += 1;

                tracing::debug!(
                    portal_id = portal.id,
                    hop = entry.from_hop,
                    camera_layer,
                    "Spawned portal camera"
                );
            }
        }
    }
}
