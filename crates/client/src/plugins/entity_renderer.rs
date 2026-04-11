//! Renders replicated game entities (players, NPCs, boss) as PBR capsule meshes.
//!
//! Every entity that arrives from the server with a `WorldPosition` + `Replicated`
//! gets a capsule mesh inserted directly.  A separate system keeps the Bevy
//! `Transform` in sync as the server pushes position updates.
//!
//! # Coordinate mapping
//! Same convention as `tile_renderer`: world (x, y, z_elevation) → Bevy (x, z_elevation, y).
//! The capsule center is placed half a unit above the tile surface so the
//! entity visually stands on the ground.

use bevy::prelude::*;
use fellytip_shared::components::WorldPosition;
use lightyear::prelude::Replicated;

pub struct EntityRendererPlugin;

impl Plugin for EntityRendererPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_entity_assets)
            .add_systems(Update, (spawn_entity_visuals, sync_entity_transforms));
    }
}

// ── Assets ────────────────────────────────────────────────────────────────────

/// Shared mesh + material handles for entity visuals.  Pre-built at startup.
#[derive(Resource)]
struct EntityVisualAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

fn setup_entity_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Capsule: radius 0.3, half-cylinder length 0.4 → total height ≈ 1.4 units.
    let mesh = meshes.add(Capsule3d::new(0.3, 0.4));
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.95, 0.75, 0.20), // warm gold — visible against terrain
        perceptual_roughness: 0.55,
        ..default()
    });
    commands.insert_resource(EntityVisualAssets { mesh, material });
}

// ── Coordinate helper ─────────────────────────────────────────────────────────

/// Convert a `WorldPosition` to a Bevy `Vec3`, placing the entity centre
/// `CAPSULE_HALF_HEIGHT` above the tile surface.
const CAPSULE_HALF_HEIGHT: f32 = 0.7; // half of total capsule height (~1.4 / 2)

fn world_to_bevy(pos: &WorldPosition) -> Vec3 {
    Vec3::new(pos.x, pos.z + CAPSULE_HALF_HEIGHT, pos.y)
}

// ── Systems ───────────────────────────────────────────────────────────────────

type NewReplicatedPos = (Added<WorldPosition>, With<Replicated>);
type ChangedReplicatedPos = (Changed<WorldPosition>, With<Replicated>);

/// Fires once per entity the first time `WorldPosition` is added by replication.
/// Inserts the visual components directly onto the replicated entity.
fn spawn_entity_visuals(
    mut commands: Commands,
    assets: Res<EntityVisualAssets>,
    query: Query<(Entity, &WorldPosition), NewReplicatedPos>,
) {
    for (entity, pos) in &query {
        commands.entity(entity).insert((
            Transform::from_translation(world_to_bevy(pos)),
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(assets.material.clone()),
        ));
    }
}

/// Keeps the Bevy `Transform` in sync whenever the server pushes a position update.
fn sync_entity_transforms(
    mut query: Query<(&WorldPosition, &mut Transform), ChangedReplicatedPos>,
) {
    for (pos, mut transform) in &mut query {
        transform.translation = world_to_bevy(pos);
    }
}
